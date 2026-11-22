//! Real pane rendering and key dispatch without booting an engine.
use super::*;
use gpui::{AppContext, TestAppContext, VisualTestContext};
use zuse_protocol::presentation::zapper_panel::ZapperPage;

struct NavigationHost {
    shell: Entity<Shell>,
    _data_dir: tempfile::TempDir,
}

impl Render for NavigationHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.update(cx, |shell, cx| {
            shell
                .navigation_focus
                .remember(&shell.shortcut_focus, window, cx);
            let root = shell.shortcut_focus.clone();
            let (preferred, neutral) = shell.navigation_focus_fallback(cx);
            window.defer(cx, move |window, cx| {
                restore_mounted_focus(&root, &preferred, &neutral, window, cx);
            });
            let main = shell.render_main(window, 400., 400., cx);
            // A Zapper coordinator chat's right side is its Zapper panel.
            let zapper_panel = shell.zapper_panel_on_right(cx) && !shell.zapper_ui.panel_hidden;
            let right = if zapper_panel {
                shell.render_zapper_inspector(window, cx)
            } else {
                shell.render_right_pane(window, cx)
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .track_focus(&shell.shortcut_focus)
                .child(div().track_focus(&shell.unfocused))
                .on_action(cx.listener(|shell, command: &DesktopCommand, window, cx| {
                    shell.desktop_command(command.0, window, cx);
                }))
                .on_action(cx.listener(|shell, _: &NextSession, window, cx| {
                    shell.cycle_navigation(true, window, cx);
                }))
                .on_action(cx.listener(|shell, _: &PrevSession, window, cx| {
                    shell.cycle_navigation(false, window, cx);
                }))
                .child(div().h(px(40.)).child(shell.render_right_tab_strip(cx)))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(
                            div()
                                .id("navigation-main")
                                .debug_selector(|| "navigation-main".into())
                                .w(px(400.))
                                .h_full()
                                .child(main),
                        )
                        .when(zapper_panel || shell.right_pane_open(cx), |el| {
                            el.child(right)
                        }),
                )
        })
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
        settings::init(UiSettings::default(), dir.path(), cx);
        crate::history::init(
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            cx,
        );
        apply_keymap(
            cx,
            &KeymapConfig::default(),
            ComposerSendBehavior::default(),
        );
    });
    let (host, cx) = cx.add_window_view(|_, cx| {
        let shell = cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            let mut shell = Shell::new(
                state,
                EngineBootConfig {
                    target: None,
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zuse_protocol::HarnessId::Mock,
                },
                cx,
            );
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            shell.state.update(cx, |state, _| {
                state.workspace_scope = Some(WorkspaceScope::Local);
                state.local_device_id = Some("local".into());
                state.no_project = true;
                state.chats = ["parent", "other"]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, id)| {
                        serde_json::from_value(serde_json::json!({
                            "id": id, "title": id, "deviceId": "local", "archived": false,
                            "createdAt": Utc::now() - chrono::Duration::minutes(ix as i64),
                        }))
                        .unwrap()
                    })
                    .collect();
                state.selected_chat = Some("parent".into());
            });
            shell.active_chat = "parent".into();
            shell.reduced_motion = true;
            for doc in ["first", "second"] {
                shell.add_subagent_surface("parent".into(), doc.into(), doc.into(), false, cx);
            }
            shell
                .composer
                .update(cx, |composer, _| composer.focus_pending = false);
            shell
        });
        NavigationHost {
            shell,
            _data_dir: dir,
        }
    });
    let shell = host.read_with(cx, |host, _| host.shell.clone());
    cx.update(|window, cx| window.draw(cx).clear());
    (shell, cx)
}

fn press(shell: &Entity<Shell>, cx: &mut VisualTestContext, forward: bool) {
    use settings::keybindings::Command;
    let right = cx.update(|window, cx| shell.read(cx).navigation_focus.in_right(window, cx));
    let command = match (right, forward) {
        (true, true) => Command::NextPanel,
        (true, false) => Command::PrevPanel,
        (false, true) => Command::NextChat,
        (false, false) => Command::PrevChat,
    };
    cx.simulate_keystrokes(&platform_combo(command.default_combo()));
    cx.update(|window, cx| window.draw(cx).clear());
}

fn assert_surface(shell: &Entity<Shell>, cx: &mut VisualTestContext, surface: RightSurface) {
    cx.update(|window, cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.resolved_right_active(cx), surface);
        assert_eq!(
            shell.state.read(cx).selected_chat.as_deref(),
            Some("parent")
        );
        assert!(shell.navigation_focus.in_right(window, cx));
    });
}

#[gpui::test]
fn repeated_keys_stay_in_read_only_pane_and_follow_reordering(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    for expected in [2, 1, 2, 1] {
        press(&shell, cx, true);
        assert_surface(&shell, cx, RightSurface::Subagent(expected));
    }
    shell.update(cx, |shell, cx| {
        shell.add_subagent_surface("parent".into(), "third".into(), "Third".into(), false, cx);
        shell.reorder_right_tabs(2, 0, cx);
    });
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    press(&shell, cx, false);
    assert_surface(&shell, cx, RightSurface::Subagent(3));
    press(&shell, cx, false);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
}

#[gpui::test]
fn clicking_main_transcript_switches_back_to_session_navigation(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    press(&shell, cx, true);
    let main = cx.debug_bounds("navigation-main").unwrap();
    cx.simulate_click(
        main.origin + gpui::point(px(100.), px(150.)),
        gpui::Modifiers::default(),
    );
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn side_chat_to_diff_retains_right_focus_and_custom_bindings(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let chat = serde_json::from_value(serde_json::json!({
                "id": "side", "parentChatId": "parent", "deviceId": "local",
                "archived": false, "createdAt": Utc::now(),
            }))
            .unwrap();
            shell.open_side_chat(chat, shell.panel_key(cx), cx);
            shell.add_diff_surface(window, cx);
            shell.activate_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(
            shell.read(cx).side_chats[&1]
                .composer
                .focus_handle(cx)
                .is_focused(window)
        );
        let mut keymap = KeymapConfig::default();
        keymap.set(
            ShortcutId::Desktop(settings::keybindings::Command::NextPanel),
            "alt-down".into(),
        );
        keymap.set(
            ShortcutId::Desktop(settings::keybindings::Command::PrevPanel),
            "alt-up".into(),
        );
        apply_keymap(cx, &keymap, ComposerSendBehavior::default());
    });
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    cx.simulate_keystrokes("alt-up");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::SideChat(1), window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    // Closing returns to the most recently visited tab, then cycles in strip order.
    assert_surface(&shell, cx, RightSurface::Diff(1));
    cx.simulate_keystrokes("alt-down");
    cx.update(|window, cx| window.draw(cx).clear());
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

#[gpui::test]
fn closing_active_and_last_tabs_recovers_the_correct_pane(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::Subagent(1), window, cx);
        })
    });
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(RightSurface::Subagent(2), window, cx);
            assert!(!shell.right_pane_open(cx));
            assert!(shell.composer.focus_handle(cx).is_focused(window));
        })
    });
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn modal_and_picker_block_navigation_in_both_panes(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    for right in [false, true] {
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                let focus = if right {
                    &shell.navigation_focus.right
                } else {
                    &shell.navigation_focus.main
                };
                window.focus(focus, cx);
                shell.delete_confirm = Some("parent".into());
            })
        });
        press(&shell, cx, true);
        press(&shell, cx, false);
        shell.update(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("parent")
            );
            assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
            shell.delete_confirm = None;
        });
    }
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell
                .composer
                .update(cx, |composer, cx| composer.open_model_menu(window, cx));
        })
    });
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(
            shell.state.read(cx).selected_chat.as_deref(),
            Some("parent")
        );
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
    });
}

#[gpui::test]
fn empty_picker_and_stale_tabs_do_not_change_sessions(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let key = shell.panel_key(cx);
            shell.right_tabs.insert(key, vec![RightSurface::File(999)]);
            shell.set_right_active(RightSurface::Picker, cx);
            window.focus(&shell.navigation_focus.right, cx);
        })
    });
    press(&shell, cx, true);
    press(&shell, cx, false);
    assert_surface(&shell, cx, RightSurface::Picker);
}

#[gpui::test]
fn hiding_reopening_and_expanding_preserve_navigation_boundaries(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    shell.update(cx, |shell, cx| shell.toggle_right_pane_expand(cx));
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    shell.update(cx, |shell, cx| shell.set_surfaces_open(false, cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(!shell.read(cx).navigation_focus.in_right(window, cx));
        assert!(shell.read(cx).composer.focus_handle(cx).is_focused(window));
    });
    shell.update(cx, |shell, cx| shell.set_surfaces_open(true, cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(shell.read(cx).composer.focus_handle(cx).is_focused(window));
    });
    // Restored tabs do not take navigation away from the main composer.
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn embedded_terminal_and_browser_focus_route_to_right_tabs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        // No engine in this harness: reserve a real emulator tab without
        // starting a PTY, then use the normal surface activation path.
        shell.right_terminal_panel(cx).update(cx, |panel, cx| {
            panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
        });
        shell.add_terminal_surface(cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        let terminal = shell.read(cx).right_terminal.as_ref().unwrap().read(cx);
        assert!(terminal.focus_handle().is_focused(window));
    });
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.add_browser_surface(None, window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
    // Exercise the GPUI target the native browser bridge focuses before it
    // redispatches a shortcut (the native OS event itself needs platform QA).
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_right_active(RightSurface::Browser(1), cx);
            window.focus(&shell.browsers[&1].focus_handle(cx), cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

#[gpui::test]
fn bottom_terminal_keeps_session_navigation(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.terminal_panel(cx).update(cx, |panel, cx| {
                panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
            });
            shell.toggle_terminal(window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        assert!(
            shell
                .read(cx)
                .terminal
                .as_ref()
                .unwrap()
                .read(cx)
                .focus_handle()
                .is_focused(window)
        );
    });
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
    });
}

#[gpui::test]
fn each_session_cycles_only_its_own_tabs(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_chat("other".into(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.add_subagent_surface("other".into(), "third".into(), "Third".into(), false, cx);
            shell.activate_right_surface(RightSurface::Subagent(3), window, cx);
        })
    });
    press(&shell, cx, true);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("other"));
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(3));
    });
    shell.update(cx, |shell, cx| shell.open_chat("parent".into(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    let tab = cx.debug_bounds("right-surface-tab-0").unwrap().center();
    cx.simulate_click(tab, gpui::Modifiers::default());
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(2));
    press(&shell, cx, true);
    assert_surface(&shell, cx, RightSurface::Subagent(1));
}

/// Settles a transcript selection and seeds the clipboard with a sentinel, so
/// a keystroke that copies nothing is distinguishable from one that does.
fn select_transcript_text(cx: &mut VisualTestContext) {
    crate::markdown::selection::begin_with_span("transcript-row:0", "selected reply", 0..8);
    crate::markdown::selection::end_active_drag();
    cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into())));
}

fn clipboard_text(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn clear_transcript_selection() {
    crate::markdown::selection::clear_if_owner("transcript-row:0");
}

#[gpui::test]
fn copy_shortcut_copies_transcript_selection_after_clicking_the_transcript(
    cx: &mut TestAppContext,
) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| window.focus(&shell.read(cx).composer.focus_handle(cx), cx));
    let main = cx.debug_bounds("navigation-main").unwrap();
    cx.simulate_click(
        main.origin + gpui::point(px(100.), px(150.)),
        gpui::Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        let shell = shell.read(cx);
        assert!(!shell.composer.focus_handle(cx).is_focused(window));
        assert!(shell.navigation_focus.main.is_focused(window));
    });
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("selected"));
    clear_transcript_selection();
}

#[gpui::test]
fn copy_shortcut_copies_side_chat_selection_from_the_right_pane(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        let right = shell.read(cx).navigation_focus.right.clone();
        window.focus(&right, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("selected"));
    clear_transcript_selection();
}

#[gpui::test]
fn copy_shortcut_prefers_the_focused_composer_selection(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| window.focus(&shell.read(cx).composer.focus_handle(cx), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    cx.simulate_input("draft text");
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-a ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("draft text"));
    clear_transcript_selection();
}

#[gpui::test]
fn ctrl_c_in_a_terminal_is_not_taken_by_a_transcript_selection(cx: &mut TestAppContext) {
    let _selection = crate::markdown::selection::test_state_lock();
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.terminal_panel(cx).update(cx, |panel, cx| {
                panel.reserve_tab_for_chat("parent".into(), "Test terminal", cx);
            });
            shell.toggle_terminal(window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear());
    select_transcript_text(cx);
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(clipboard_text(cx).as_deref(), Some("sentinel"));
    clear_transcript_selection();
}

#[gpui::test]
fn pull_requests_from_a_chat_open_as_tabs_beside_the_focused_session(cx: &mut TestAppContext) {
    use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
    let (shell, cx) = setup(cx);
    let link = |url: &str| LinkActivation {
        target: LinkTarget::new(url, url),
        action: LinkAction::Primary,
        source_session: Some("parent".into()),
    };
    let pull_requests = |shell: &Shell| -> Vec<String> {
        shell.right_tabs["parent"]
            .iter()
            .filter_map(|surface| match surface {
                RightSurface::PullRequest(id) => Some(shell.chat_pull_requests[id].url.clone()),
                _ => None,
            })
            .collect()
    };
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            // A link into a pull request opens its view in this session's pane.
            assert_eq!(
                shell.activate_session_link(
                    &link("https://github.com/a/b/pull/1/files"),
                    window,
                    cx
                ),
                LinkOutcome::Internal
            );
            // Another session's sidebar badge opens here too, so pull requests
            // can be compared side by side; the same one reuses its tab.
            shell.open_chat_pull_request("https://github.com/c/d/pull/2".into(), None, window, cx);
            shell.activate_session_link(&link("https://github.com/a/b/pull/1"), window, cx);
            assert_eq!(
                pull_requests(shell),
                [
                    "https://github.com/a/b/pull/1",
                    "https://github.com/c/d/pull/2"
                ]
            );
            assert!(matches!(shell.route, Route::Chat));
            assert!(shell.right_pane_open(cx));
            let active = shell.resolved_right_active(cx);
            let RightSurface::PullRequest(id) = active else {
                panic!("a pull request tab is active: {active:?}");
            };
            assert_eq!(
                shell.chat_pull_requests[&id].url,
                "https://github.com/a/b/pull/1"
            );
            // Closing the tab drops its view.
            shell.close_right_surface(active, window, cx);
            assert!(!shell.chat_pull_requests.contains_key(&id));
        });
    });
    // Pull requests sent to the browser in Settings keep opening there.
    cx.update(|window, cx| {
        settings::update(SavePolicy::Immediate, cx, |settings| {
            settings.pull_request_destination = settings::PullRequestDestination::External;
            // Keep the link out of an embedded browser, which a test can't host.
            settings.open_web_links_in_zuse = false;
        });
        shell.update(cx, |shell, cx| {
            let before = shell.chat_pull_requests.len();
            assert_eq!(
                shell.activate_session_link(&link("https://github.com/e/f/pull/3"), window, cx),
                LinkOutcome::External("https://github.com/e/f/pull/3".into())
            );
            assert_eq!(shell.chat_pull_requests.len(), before);
        });
    });
}

#[gpui::test]
fn find_shortcut_searches_the_focused_side_chat(cx: &mut TestAppContext) {
    use settings::keybindings::Command;
    let (shell, cx) = setup(cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let chat = serde_json::from_value(serde_json::json!({
                "id": "side", "parentChatId": "parent", "deviceId": "local",
                "archived": false, "createdAt": Utc::now(),
            }))
            .unwrap();
            shell.open_side_chat(chat, shell.panel_key(cx), cx);
            shell.activate_right_surface(RightSurface::SideChat(1), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear());
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            let (_, transcript, composer) = shell.shortcut_conversation(window, cx);
            assert_ne!(composer, shell.composer);
            shell.desktop_command(Command::FindInChat, window, cx);
            assert!(transcript.read(cx).find_open());
            assert!(!shell.transcript.read(cx).find_open());
        });
    });
}

#[gpui::test]
fn zapper_page_is_a_route_that_chats_and_history_can_leave(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_new_zapper(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("zapper-page").is_some());

    // Opening a chat from the sidebar leaves the form.
    shell.update(cx, |shell, cx| shell.open_chat("parent".into(), cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("zapper-page").is_none());
    shell.read_with(cx, |shell, _| {
        assert!(matches!(shell.route, Route::Chat));
        assert_eq!(shell.nav.current(), &NavEntry::Chat("parent".into()));
        assert!(shell.nav.can_back());
    });

    // Back returns to the same draft, Forward to the chat.
    shell.update(cx, |shell, cx| {
        shell.navigate_back(cx);
        assert!(matches!(shell.route, Route::Zapper));
        assert!(shell.creating_zapper());
        shell.navigate_forward(cx);
        assert!(matches!(shell.route, Route::Chat));
    });
}

#[gpui::test]
fn cancelling_a_zapper_page_returns_to_the_conversation(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.open_chat("parent".into(), cx);
        shell.open_new_zapper(cx);
        assert!(shell.zapper_ui.page.is_some() || matches!(shell.route, Route::Zapper));
    });
    cx.run_until_parked();
    shell.update(cx, |shell, cx| {
        shell.leave_zapper_page(cx);
        assert!(shell.zapper_ui.page.is_none());
        assert!(matches!(shell.route, Route::Chat));
        assert_eq!(
            shell.state.read(cx).selected_chat.as_deref(),
            Some("parent")
        );
    });
}

#[gpui::test]
fn the_right_pane_toggle_closes_the_zapper_panel(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        let zapper = serde_json::from_value(serde_json::json!({
            "id": "zapper", "revision": 1, "status": "active",
            "coordinatorChatId": "parent", "previousCoordinatorChats": [],
            "workers": [], "decisions": [],
            "config": {
                "name": "Captain", "purpose": "", "homeDeviceId": "d",
                "repositories": [], "workerDeviceId": "d",
                "coordinatorModel": { "harness": "codex", "model": null },
                "maxConcurrentWorkers": 4, "maxFollowups": 3
            }
        }))
        .unwrap();
        shell
            .state
            .update(cx, |state, _| state.zappers = vec![zapper]);
        assert!(shell.selected_zapper(cx).is_some());
        assert!(!shell.zapper_ui.panel_hidden);
        // The setup's subagent tabs stand in for the panel: the first toggle
        // closes them back to it.
        assert!(!shell.zapper_panel_on_right(cx));
        shell.toggle_right_pane(cx);
        assert!(shell.zapper_panel_on_right(cx));
        assert!(!shell.zapper_ui.panel_hidden);
        shell.toggle_right_pane(cx);
        assert!(shell.zapper_ui.panel_hidden);
        shell.toggle_right_pane(cx);
        assert!(!shell.zapper_ui.panel_hidden);
    });
}

#[gpui::test]
fn pending_delete_dialog_can_be_dismissed_without_releasing_submission_guard(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.delete_confirm = Some("parent".into());
        shell.deleting_chat = true;
        assert!(shell.capture_escape_surface(cx));
        assert!(shell.delete_confirm.is_none());
        assert!(shell.deleting_chat);
    });
}

fn coordinate_zapper_from(shell: &mut Shell, chat: &str, cx: &mut Context<Shell>) {
    let zapper = serde_json::from_value(serde_json::json!({
        "id": "zapper", "revision": 1, "status": "active",
        "coordinatorChatId": chat, "previousCoordinatorChats": [],
        "workers": [], "decisions": [],
        "config": {
            "name": "Captain", "purpose": "", "homeDeviceId": "d",
            "repositories": [], "workerDeviceId": "d",
            "coordinatorModel": { "harness": "codex", "model": null },
            "maxConcurrentWorkers": 4, "maxFollowups": 3
        }
    }))
    .unwrap();
    shell
        .state
        .update(cx, |state, _| state.zappers = vec![zapper]);
}

fn close_surface(shell: &Entity<Shell>, cx: &mut VisualTestContext, surface: RightSurface) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.close_right_surface(surface, window, cx)
        })
    });
}

#[gpui::test]
fn a_subagent_chip_opens_focuses_and_closes_its_right_pane_tab(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    // A closed pane opens on the chip, and the doc already open is focused,
    // not duplicated.
    shell.update(cx, |shell, cx| {
        shell.set_surfaces_open(false, cx);
        assert!(!shell.right_pane_open(cx));
        shell.add_subagent_surface("parent".into(), "first".into(), "first".into(), false, cx);
        assert!(shell.right_pane_shown(cx));
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(1));
        assert_eq!(shell.right_tabs["parent"].len(), 2);
        assert_eq!(shell.subagent_tabs.len(), 2);
    });
    close_surface(&shell, cx, RightSurface::Subagent(1));
    shell.read_with(cx, |shell, cx| {
        assert!(!shell.subagent_tabs.contains_key(&1));
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
        assert!(shell.right_pane_open(cx));
    });
    close_surface(&shell, cx, RightSurface::Subagent(2));
    shell.read_with(cx, |shell, cx| {
        assert!(shell.subagent_tabs.is_empty());
        assert!(!shell.right_pane_open(cx));
    });
}

#[gpui::test]
fn a_subagent_tab_open_in_another_chat_moves_into_this_chats_strip(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_chat("other".into(), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    shell.update(cx, |shell, cx| {
        assert_eq!(shell.active_chat, "other");
        shell.add_subagent_surface("other".into(), "first".into(), "first".into(), false, cx);
        assert_eq!(shell.subagent_tabs.len(), 2);
        assert_eq!(shell.right_tabs["other"], vec![RightSurface::Subagent(1)]);
        assert!(!shell.right_tabs["parent"].contains(&RightSurface::Subagent(1)));
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(1));
        assert!(shell.right_pane_shown(cx));
    });
}

/// Open `url` the way a transcript link or a PR chip does.
fn open_pr(shell: &Entity<Shell>, cx: &mut VisualTestContext, url: &str) {
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_chat_pull_request(url.into(), None, window, cx)
        })
    });
}

/// Open the checkout review the way the session info row, the Changes
/// command and the panel menu do.
fn open_review(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_review_tab(window, cx)));
}

/// Leave the setup on the "other" chat, coordinated by a Zapper, with the
/// Zapper panel on the right showing Overview.
fn zapper_chat(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| shell.open_chat("other".into(), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    shell.update(cx, |shell, cx| {
        assert_eq!(shell.active_chat, "other");
        coordinate_zapper_from(shell, "other", cx);
        assert!(shell.zapper_panel_on_right(cx));
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::Section(zapper_panel::ZapperTab::Overview)
        );
    });
    (shell, cx)
}

/// What a Zapper chat must keep true whatever is open: the Zapper panel is
/// the right side, the surface host stays shut, and no titlebar surface tab
/// stands for a pane nobody sees.
fn assert_zapper_panel_hosts(shell: &Shell, cx: &App) {
    assert!(shell.zapper_panel_on_right(cx));
    assert!(!shell.titlebar_trailing_layout(false, cx).right_pane_visible);
    assert!(shell.right_tabs.get("other").is_none_or(Vec::is_empty));
}

#[gpui::test]
fn a_pull_request_in_a_zapper_chat_opens_focuses_and_closes_as_a_zapper_panel_tab(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = zapper_chat(cx);
    let first = "https://github.com/a/b/pull/1";
    let second = "https://github.com/a/b/pull/2";
    open_pr(&shell, cx, first);
    cx.update(|window, cx| window.draw(cx).clear());
    shell.read_with(cx, |shell, cx| {
        assert_zapper_panel_hosts(shell, cx);
        // A tab after the sections; Overview and the rest stay put.
        assert_eq!(
            shell.zapper_tabs().open_tabs(),
            [ZapperPage::PullRequest(first.into())]
        );
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::PullRequest(first.into())
        );
        assert_eq!(
            shell.zapper_tabs().section(),
            zapper_panel::ZapperTab::Overview
        );
        assert!(shell.chat_pull_requests.is_empty());
    });

    // Another link opens beside it; the first link focuses its tab.
    open_pr(&shell, cx, second);
    open_pr(&shell, cx, first);
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.zapper_tabs().open_tabs().len(), 2);
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::PullRequest(first.into())
        );
    });

    // A hidden panel comes back for the link.
    shell.update(cx, |shell, cx| {
        shell.toggle_right_pane(cx);
        assert!(shell.zapper_ui.panel_hidden);
    });
    open_pr(&shell, cx, second);
    shell.update(cx, |shell, cx| {
        assert!(!shell.zapper_ui.panel_hidden);
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::PullRequest(second.into())
        );

        // Closing goes back to the page shown before; the last one to
        // Overview.
        shell.close_zapper_page(&ZapperPage::PullRequest(second.into()), cx);
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::PullRequest(first.into())
        );
        shell.close_zapper_page(&ZapperPage::PullRequest(first.into()), cx);
        assert!(shell.zapper_tabs().open_tabs().is_empty());
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::Section(zapper_panel::ZapperTab::Overview)
        );
        assert_zapper_panel_hosts(shell, cx);
    });
}

#[gpui::test]
fn zapper_panel_tabs_sit_right_after_the_section_icons(cx: &mut TestAppContext) {
    let (shell, cx) = zapper_chat(cx);
    let url = "https://github.com/a/b/pull/7";
    open_pr(&shell, cx, url);
    cx.update(|window, cx| window.draw(cx).clear());
    // With the pull request active every section is an icon; the last one
    // shown (Drive hides until written) ends just before the tab.
    let last_section = [
        "zapper-tab-Drive",
        "zapper-tab-Threads",
        "zapper-tab-Issues",
        "zapper-tab-Overview",
    ]
    .into_iter()
    .find_map(|selector| cx.debug_bounds(selector))
    .expect("a section button is drawn");
    let tab = cx
        .debug_bounds("zapper-page-pr-https://github.com/a/b/pull/7")
        .expect("the pull request tab is drawn");
    let gap = tab.left() - last_section.right();
    // The separator and the gaps around it, no reserved room.
    assert!(gap > px(0.0) && gap < px(16.0), "gap {gap:?}");
}

#[gpui::test]
fn the_review_diff_in_a_zapper_chat_opens_focuses_and_closes_as_a_zapper_panel_tab(
    cx: &mut TestAppContext,
) {
    let (shell, cx) = zapper_chat(cx);
    open_review(&shell, cx);
    cx.update(|window, cx| window.draw(cx).clear());
    let review = shell.read_with(cx, |shell, cx| {
        assert_zapper_panel_hosts(shell, cx);
        let review = shell.review_surface(cx).expect("review is open");
        assert_eq!(shell.zapper_tabs().open_tabs(), [ZapperPage::Diff(review)]);
        assert_eq!(shell.zapper_tabs().active(), &ZapperPage::Diff(review));
        review
    });
    // Reopening focuses the same diff, from another page too.
    shell.update(cx, |shell, cx| {
        shell.show_zapper_page(ZapperPage::Section(zapper_panel::ZapperTab::Issues), cx)
    });
    open_review(&shell, cx);
    shell.update(cx, |shell, cx| {
        assert_eq!(shell.zapper_tabs().open_tabs(), [ZapperPage::Diff(review)]);
        assert_eq!(shell.zapper_tabs().active(), &ZapperPage::Diff(review));

        // Closing drops the diff and its watch and returns to Issues.
        shell.close_zapper_page(&ZapperPage::Diff(review), cx);
        assert!(!shell.diffs.contains_key(&review));
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::Section(zapper_panel::ZapperTab::Issues)
        );
        assert_zapper_panel_hosts(shell, cx);
    });
}

#[gpui::test]
fn a_subagent_chip_in_a_zapper_chat_opens_as_a_zapper_panel_tab(cx: &mut TestAppContext) {
    let (shell, cx) = zapper_chat(cx);
    shell.update(cx, |shell, cx| {
        shell.add_subagent_surface("other".into(), "zsub".into(), "Research".into(), false, cx);
        assert_zapper_panel_hosts(shell, cx);
        assert_eq!(shell.zapper_tabs().open_tabs(), [ZapperPage::Subagent(3)]);
        assert_eq!(shell.zapper_tabs().active(), &ZapperPage::Subagent(3));
        assert_eq!(shell.subagent_tabs[&3].doc_id, "zsub");

        // Reopening focuses it rather than adding another.
        shell.show_zapper_page(ZapperPage::Section(zapper_panel::ZapperTab::Threads), cx);
        shell.add_subagent_surface("other".into(), "zsub".into(), "Research".into(), false, cx);
        assert_eq!(shell.zapper_tabs().open_tabs().len(), 1);
        assert_eq!(shell.subagent_tabs.len(), 3);
        assert_eq!(shell.zapper_tabs().active(), &ZapperPage::Subagent(3));
    });
    shell.update(cx, |shell, cx| {
        // Closing drops the transcript and returns to Threads.
        shell.close_zapper_page(&ZapperPage::Subagent(3), cx);
        assert!(!shell.subagent_tabs.contains_key(&3));
        assert_eq!(
            shell.zapper_tabs().active(),
            &ZapperPage::Section(zapper_panel::ZapperTab::Threads)
        );
        assert_zapper_panel_hosts(shell, cx);
    });
}

#[gpui::test]
fn a_pull_request_and_a_diff_in_a_plain_chat_still_open_in_the_right_pane(cx: &mut TestAppContext) {
    let (shell, cx) = setup(cx);
    shell.update(cx, |shell, cx| {
        shell.set_surfaces_open(false, cx);
        assert!(shell.selected_zapper(cx).is_none());
    });
    open_pr(&shell, cx, "https://github.com/a/b/pull/3");
    open_review(&shell, cx);
    shell.read_with(cx, |shell, cx| {
        assert!(shell.right_pane_shown(cx));
        assert!(!shell.zapper_panel_on_right(cx));
        let review = shell.review_surface(cx).expect("review is open");
        assert_eq!(shell.resolved_right_active(cx), RightSurface::Diff(review));
        // The setup's two subagents, the pull request and the review.
        assert_eq!(shell.right_tabs["parent"].len(), 4);
    });
}

/// Give the setup's Zapper issue ZAP-18 with pull request `url` linked.
fn add_issue_with_pull_request(shell: &mut Shell, url: &str, cx: &mut Context<Shell>) {
    let issue: zuse_protocol::zappers::ZapperIssue = serde_json::from_value(serde_json::json!({
        "id": "issue-18", "number": 18, "createdAt": 0, "updatedAt": 0,
        "details": { "title": "Rename", "description": "", "dueDate": null },
        "threads": [], "activity": [],
        "pullRequests": [{ "url": url, "title": "Rename it" }]
    }))
    .unwrap();
    shell
        .state
        .update(cx, |state, _| state.zappers[0].issues = vec![issue]);
}

/// The issue surface notifies for its own reasons (hover, a sync, the click
/// that opened a pull request); none of them may pull the panel back to the
/// issue it last showed.
fn notify_issue_surface(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let panel = shell.read_with(cx, |shell, _| shell.zapper_ui.issues.clone().unwrap());
    panel.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

#[gpui::test]
fn a_pull_request_tab_beside_its_issue_tab_shows_the_pull_request(cx: &mut TestAppContext) {
    let (shell, cx) = zapper_chat(cx);
    let url = "https://github.com/a/b/pull/315";
    let pr = ZapperPage::PullRequest(url.into());
    let issue = ZapperPage::Issue("issue-18".into());
    shell.update(cx, |shell, cx| {
        add_issue_with_pull_request(shell, url, cx);
        shell.show_in_zapper_panel(issue.clone(), cx);
    });
    cx.update(|window, cx| window.draw(cx).clear());

    // The pull request row on the issue page opens the pull request.
    let panel = shell.read_with(cx, |shell, cx| {
        let panel = shell.zapper_ui.issues.clone().expect("issue surface drawn");
        assert_eq!(panel.read(cx).selected_id(), Some("issue-18"));
        panel
    });
    panel.update(cx, |_, cx| {
        cx.emit(zuse_viewport::zapper_issues::IssueEvent::OpenPullRequest(
            url.into(),
        ));
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
    notify_issue_surface(&shell, cx);
    shell.read_with(cx, |shell, cx| {
        assert_zapper_panel_hosts(shell, cx);
        assert_eq!(shell.zapper_tabs().open_tabs(), [issue.clone(), pr.clone()]);
        assert_eq!(shell.zapper_tabs().active(), &pr);
    });

    // The issue tab shows the issue again.
    shell.update(cx, |shell, cx| shell.show_zapper_page(issue.clone(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    notify_issue_surface(&shell, cx);
    shell.read_with(cx, |shell, cx| {
        assert_eq!(shell.zapper_tabs().active(), &issue);
        assert_eq!(panel.read(cx).selected_id(), Some("issue-18"));
    });

    // The pull request tab shows the pull request, and stays on it.
    shell.update(cx, |shell, cx| shell.show_zapper_page(pr.clone(), cx));
    cx.update(|window, cx| window.draw(cx).clear());
    notify_issue_surface(&shell, cx);
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.zapper_tabs().active(), &pr);
        assert_eq!(shell.zapper_tabs().open_tabs().len(), 2);
    });

    // A row in the issue surface still opens its issue as a tab.
    shell.update(cx, |shell, cx| {
        shell.show_zapper_page(ZapperPage::Section(zapper_panel::ZapperTab::Issues), cx)
    });
    cx.update(|window, cx| window.draw(cx).clear());
    panel.update(cx, |panel, cx| panel.show_issue("issue-18".into(), cx));
    cx.run_until_parked();
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.zapper_tabs().active(), &issue)
    });
}
