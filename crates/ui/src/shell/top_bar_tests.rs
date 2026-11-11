use super::*;
use gpui::{TestAppContext, VisualTestContext};

#[test]
fn provider_brand_colors_are_consistent_across_pickers() {
    use zuse_protocol::HarnessId;
    for harness in [
        HarnessId::Codex,
        HarnessId::ClaudeCode,
        HarnessId::Gemini,
        HarnessId::Kiro,
        HarnessId::Zuse,
    ] {
        assert!(
            crate::pickers::harness_brand_icon(harness).1.is_some(),
            "{harness:?}"
        );
    }
    let codex = crate::pickers::harness_brand_icon(HarnessId::Codex)
        .1
        .unwrap();
    let purple: gpui::Hsla = gpui::rgb(0x8b5cf6).into();
    assert_eq!((codex.h, codex.s, codex.l), (purple.h, purple.s, purple.l));
    assert!(
        crate::pickers::harness_brand_icon(HarnessId::Cursor)
            .1
            .is_none()
    );
}

struct SidebarHeadings;

impl Render for SidebarHeadings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div().size_full().flex().flex_col().children([
            spaces::sidebar_section_heading(theme, "Zappers", "test-zappers-label").child(
                spaces::sidebar_header_button("test-zappers-plus", icons::PLUS, theme)
                    .debug_selector(|| "test-zappers-plus".into()),
            ),
            spaces::sidebar_section_heading(theme, "Threads", "test-threads-label").child(
                spaces::sidebar_header_button("test-threads-plus", icons::PLUS, theme)
                    .debug_selector(|| "test-threads-plus".into()),
            ),
        ])
    }
}

#[gpui::test]
fn sidebar_section_labels_and_buttons_share_geometry(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
    });
    let (_, cx) = cx.add_window_view(|_, _| SidebarHeadings);
    cx.update(|window, cx| window.draw(cx).clear());
    let zappers = cx.debug_bounds("test-zappers-label").unwrap();
    let threads = cx.debug_bounds("test-threads-label").unwrap();
    let zappers_plus = cx.debug_bounds("test-zappers-plus").unwrap();
    let threads_plus = cx.debug_bounds("test-threads-plus").unwrap();
    assert_eq!(zappers.left(), threads.left());
    assert_eq!(zappers.size, threads.size);
    assert_eq!(zappers_plus.left(), threads_plus.left());
    assert_eq!(zappers_plus.size, threads_plus.size);
}

#[test]
fn branch_errors_explain_worktree_conflicts_and_preserve_other_details() {
    let message = format_branch_error(
        "main",
        "git: fatal: 'main' is already used by worktree at '/tmp/main worktree'",
    );
    assert!(message.contains("Branch already in use"));
    assert!(message.contains("/tmp/main worktree"));
    assert!(message.contains("Switch that worktree"));
    assert!(
        format_branch_error("feature", "local changes would be overwritten")
            .contains("Commit or stash")
    );
    assert!(format_branch_error("feature", "permission denied").contains("permission denied"));
}

#[gpui::test]
fn branch_label_uses_current_checkout_not_run_start_context(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.branch_menu.loaded_for = Some("local:/repo".into());
        shell.branch_menu.current_branch = Some("main".into());
        assert_eq!(shell.branch_target(cx).unwrap().branch, "main");
        shell.branch_menu.loaded_for = Some("local:/another".into());
        assert_eq!(
            shell.branch_target(cx).unwrap().branch,
            "feature/a-long-branch-name-to-test-truncation"
        );
    });
}

struct TabHost {
    shell: Entity<Shell>,
    _directory: tempfile::TempDir,
}

impl Render for TabHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(shell.render_overview_tabs(cx))
                .child(shell.render_thread_conversations(cx))
                .child(div().flex_1())
                .child(shell.composer.clone())
        })
    }
}

#[gpui::test]
fn conversations_and_add_button_precede_other_tabs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let (chat_id, diff_id) = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let chat_id = shell.state.read(cx).selected_chat.clone().unwrap();
            shell.active_chat = chat_id.clone();
            shell.open_review_tab(window, cx);
            (chat_id, shell.active_main_diff(cx).unwrap())
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    let chat = cx
        .debug_bounds(Box::leak(
            format!("thread-conversation-{chat_id}").into_boxed_str(),
        ))
        .unwrap();
    let add = cx.debug_bounds("add-thread-conversation").unwrap();
    let diff = cx
        .debug_bounds(Box::leak(
            format!("main-tab-diff-{diff_id}").into_boxed_str(),
        ))
        .unwrap();
    assert!(chat.right() <= add.left());
    assert!(add.right() <= diff.left());
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let directory = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), directory.path(), cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        let shell = cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                let mut chat =
                    crate::shell::tests::chat_with_path(Some("/repo"), Some(("/repo", "/repo")));
                chat.device_id = "local".into();
                chat.source_context.as_mut().unwrap().branch =
                    "feature/a-long-branch-name-to-test-truncation".into();
                state.local_device_id = Some("local".into());
                state.no_project = true;
                state.selected_chat = Some(chat.id.clone());
                state.chats.push(chat);
                state
                    .checkout_diff_stats
                    .push(crate::state::CheckoutDiffStat {
                        checkout_id: "checkout".into(),
                        device_id: "local".into(),
                        cwd: "/repo".into(),
                        additions: 2,
                        deletions: 5,
                        files: 2,
                    });
                state
            });
            let mut shell = Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: directory.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            );
            shell.reduced_motion = true;
            shell
                .composer
                .update(cx, |composer, _| composer.focus_pending = false);
            shell
        });
        TabHost {
            shell,
            _directory: directory,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

#[gpui::test]
fn overview_tabs_have_room_and_keep_add_button_visible(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.add_overview_tab(OverviewTab::PullRequests, cx);
        shell.add_overview_tab(OverviewTab::Plugins, cx);
    });
    for width in [600.0, 360.0] {
        cx.simulate_resize(gpui::size(px(width), px(700.0)));
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
        let threads = cx.debug_bounds("overview-threads").unwrap();
        let plugins = cx.debug_bounds("overview-plugins").unwrap();
        let add = cx.debug_bounds("overview-add").unwrap();
        if width == 600.0 {
            assert_eq!(f32::from(threads.size.width), OVERVIEW_TAB_WIDTH);
        }
        if width == 600.0 {
            assert!(plugins.right() <= add.left());
        }
        assert!(f32::from(add.right()) <= width);
    }
}

#[gpui::test]
fn thread_overview_labels_follow_live_thread_titles(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.state.update(cx, |state, _| {
            state.chats[0].title = Some("Understand Project Purpose".into());
        });
        assert_eq!(
            shell.overview_label(&shell.overview_tabs[0], cx).as_ref(),
            "Understand Project Purpose"
        );
        shell.add_overview_tab(OverviewTab::Threads, cx);
        assert_eq!(
            shell
                .overview_label(shell.overview_tabs.last().unwrap(), cx)
                .as_ref(),
            "New threads"
        );
        shell.state.update(cx, |state, _| {
            state.chats[0].title = Some("Renamed project\nsummary".into());
        });
        assert_eq!(
            shell.overview_label(&shell.overview_tabs[0], cx).as_ref(),
            "Renamed project summary"
        );
        shell
            .state
            .update(cx, |state, cx| state.select_chat(Some("chat".into()), cx));
        assert_eq!(
            shell
                .overview_label(shell.overview_tabs.last().unwrap(), cx)
                .as_ref(),
            "Renamed project summary"
        );
        shell.state.update(cx, |state, _| {
            state.chats[0].title = None;
            state.chats[0].last_message_preview = Some("Investigate the build".into());
        });
        assert_eq!(
            shell
                .overview_label(shell.overview_tabs.last().unwrap(), cx)
                .as_ref(),
            "Investigate the build"
        );
    });
}

#[gpui::test]
fn overview_supports_duplicate_tabs_and_restores_neighbor(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.read_with(cx, |shell, _| assert_eq!(shell.overview_tabs.len(), 1));
    let add = cx.debug_bounds("overview-add").unwrap().center();
    cx.simulate_click(add, gpui::Modifiers::default());
    let requests = cx
        .debug_bounds("add-overview-pull-requests")
        .unwrap()
        .center();
    cx.simulate_click(requests, gpui::Modifiers::default());
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.route, Route::PullRequests);
        assert_eq!(shell.overview_tabs.len(), 2);
    });
    shell.update(cx, |shell, cx| {
        shell.add_overview_tab(OverviewTab::Threads, cx);
        let new_thread = shell.active_overview_tab;
        assert_eq!(
            shell
                .overview_label(shell.overview_tabs.last().unwrap(), cx)
                .as_ref(),
            "New threads"
        );
        assert!(shell.state.read(cx).selected_chat.is_none());
        shell.activate_overview_tab(0, cx);
        assert!(shell.state.read(cx).selected_chat.is_some());
        shell.activate_overview_tab(new_thread, cx);
        assert!(shell.state.read(cx).selected_chat.is_none());
        shell.add_overview_tab(OverviewTab::PullRequests, cx);
        shell.add_overview_tab(OverviewTab::Plugins, cx);
        shell.add_overview_tab(OverviewTab::Plugins, cx);
        assert_eq!(shell.overview_tabs.len(), 6);
        let last = shell.active_overview_tab;
        shell.close_overview_tab(last, cx);
        assert_eq!(shell.route, Route::Plugins);
        shell.close_overview_tab(shell.active_overview_tab, cx);
        assert_eq!(shell.route, Route::PullRequests);
        shell.close_overview_tab(shell.active_overview_tab, cx);
        assert_eq!(shell.route, Route::Chat);
        shell.close_overview_tab(new_thread, cx);
        shell.close_overview_tab(1, cx);
        shell.close_overview_tab(0, cx);
        assert_eq!(shell.overview_tabs.len(), 1);
    });
}

#[gpui::test]
fn branch_pill_aligns_with_create_pr_and_fits_compact_composer(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    for width in [436.0, 768.0] {
        cx.simulate_resize(gpui::size(px(width), px(700.0)));
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell
                    .composer
                    .update(cx, |composer, cx| composer.set_available_width(width, cx));
            });
            window.refresh();
            window.draw(cx).clear();
        });
        let branch = cx.debug_bounds("top-bar-branch").expect("branch pill");
        let create = cx.debug_bounds("composer-create-pr").expect("Create PR");
        assert!((f32::from(branch.top()) - f32::from(create.top())).abs() < 1.0);
        assert!(branch.left() >= create.right());
        let input = shell.read_with(cx, |shell, cx| {
            shell
                .composer
                .read(cx)
                .surface_bounds()
                .get()
                .expect("composer surface")
        });
        assert!(
            branch.right() <= input.right(),
            "branch {branch:?}, input {input:?}"
        );
        assert!(create.left() >= input.left());
        assert!(branch.bottom() <= input.top());
    }
}
