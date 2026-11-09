//! Desktop commands retain separate chat, tab, panel and text-editing scopes.
use super::*;
use settings::keybindings::Command;

pub(super) fn bind(cx: &mut App, keymap: &KeymapConfig) {
    for rule in keymap.desktop_bindings() {
        if rule.key.is_empty() {
            continue;
        }
        let combo = platform_combo(&rule.key);
        if Keystroke::parse(&combo).is_err() {
            continue;
        }
        let command = rule.command;
        // Composer/editor rules are scoped by their focused input, as in Electron.
        if command.context().is_none()
            && rule
                .when
                .as_deref()
                .is_some_and(|when| !settings::keybindings::evaluate_when(when).unwrap_or(false))
        {
            continue;
        }
        let customized = keymap.desktop.contains_key(&command)
            || keymap
                .desktop_rules
                .iter()
                .any(|rule| rule.command == command);
        let context = command.context();
        let binding = match command {
            Command::ComposerSubmit if customized => {
                KeyBinding::new(&combo, crate::composer::Submit, context)
            }
            Command::ComposerForceSubmit if customized => {
                KeyBinding::new(&combo, crate::composer::ModifiedSubmit, context)
            }
            Command::ComposerNewline if customized => {
                KeyBinding::new(&combo, crate::composer::Newline, context)
            }
            Command::ComposerSubmit | Command::ComposerForceSubmit | Command::ComposerNewline => {
                continue;
            }
            Command::ComposerPreviousMessage => {
                KeyBinding::new(&combo, crate::composer::PreviousMessage, context)
            }
            Command::ComposerNextMessage => {
                KeyBinding::new(&combo, crate::composer::NextMessage, context)
            }
            Command::ComposerQueueSubmit => {
                KeyBinding::new(&combo, crate::composer::QueueSubmit, context)
            }
            Command::ComposerDeleteToStart => {
                KeyBinding::new(&combo, crate::composer::DeleteToLineStart, context)
            }
            Command::ComposerDeleteToEnd => {
                KeyBinding::new(&combo, crate::composer::DeleteToLineEnd, context)
            }
            Command::ComposerDeleteWord => {
                KeyBinding::new(&combo, crate::composer::DeleteWordLeft, context)
            }
            Command::ComposerYank => KeyBinding::new(&combo, crate::composer::Yank, context),
            Command::ComposerYankPop => KeyBinding::new(&combo, crate::composer::YankPop, context),
            Command::ComposerStash => KeyBinding::new(&combo, crate::composer::StashDraft, context),
            Command::ComposerSearchHistory => {
                KeyBinding::new(&combo, crate::composer::SearchHistory, context)
            }
            Command::ComposerInterrupt => KeyBinding::new(
                &combo,
                crate::composer::InterruptAgent {
                    double_press: rule.key == "escape",
                },
                context,
            ),
            Command::NewChat => KeyBinding::new(&combo, NewSession, context),
            Command::OpenProject => KeyBinding::new(&combo, AddSpacePalette, context),
            Command::Settings => KeyBinding::new(&combo, OpenSettings, context),
            Command::ToggleLeftSidebar => KeyBinding::new(&combo, ToggleSidebar, context),
            Command::ToggleRightSidebar => KeyBinding::new(&combo, ToggleChanges, context),
            Command::ToggleTerminal => KeyBinding::new(&combo, ToggleTerminal, context),
            Command::EditorSave => KeyBinding::new(&combo, SaveFile, context),
            Command::OpenModelPicker => KeyBinding::new(&combo, OpenModelPicker, context),
            _ => KeyBinding::new(&combo, DesktopCommand(command), context),
        };
        cx.bind_keys([binding]);
    }
}

impl Shell {
    pub(super) fn desktop_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model_slot = match command {
            Command::SelectTab1 => Some(0),
            Command::SelectTab2 => Some(1),
            Command::SelectTab3 => Some(2),
            Command::SelectTab4 => Some(3),
            Command::SelectTab5 => Some(4),
            Command::SelectTab6 => Some(5),
            Command::SelectTab7 => Some(6),
            Command::SelectTab8 => Some(7),
            Command::SelectLastTab => Some(8),
            _ => None,
        };
        if let Some(slot) = model_slot {
            let pickers = self
                .open_side_chat_pickers(cx)
                .unwrap_or_else(|| self.composer.read(cx).pickers().clone());
            if pickers.update(cx, |pickers, cx| pickers.jump_model_slot(slot, cx)) {
                return;
            }
        }
        if self.navigation_overlay_open(cx)
            && !matches!(
                command,
                Command::Settings
                    | Command::OpenChatSwitcher
                    | Command::SearchFiles
                    | Command::ShowShortcuts
            )
        {
            return;
        }
        match command {
            Command::NewChat => self.open_new_session(None, cx),
            Command::NewChatWithoutProject => {
                self.open_new_session(None, cx);
                self.state
                    .update(cx, |state, cx| state.select_space(None, cx));
            }
            Command::OpenProject => window.dispatch_action(Box::new(AddSpacePalette), cx),
            Command::Settings => self.toggle_settings(cx),
            Command::CloseTab => {
                if self.close_active_surface(window, cx) {
                    return;
                }
                if let Some(chat) = self.state.read(cx).selected_chat.clone() {
                    self.close_thread_conversation(&chat, cx);
                }
            }
            Command::ToggleLeftSidebar => {
                if !matches!(self.route, Route::Settings(_)) {
                    self.toggle_sidebar(cx);
                }
            }
            Command::ToggleRightSidebar => window.dispatch_action(Box::new(ToggleChanges), cx),
            Command::ToggleTerminal => window.dispatch_action(Box::new(ToggleTerminal), cx),
            Command::FocusComposer => {
                self.activate_main_tab(None, cx);
                self.focus_composer(cx);
            }
            Command::NextChat | Command::PrevChat => {
                if self.pull_request_pane_open() && self.navigation_focus.in_right(window, cx) {
                    self.cycle_pull_request_tabs(command == Command::NextChat, window, cx);
                } else {
                    self.step_desktop_chat(command == Command::NextChat, cx);
                }
            }
            Command::NewTab => self.add_thread_conversation(cx),
            Command::NextTab | Command::PrevTab => {
                let Some((_, rows, _)) = self.thread_conversation_tabs(cx) else {
                    return;
                };
                let order: Vec<_> = rows.into_iter().map(|chat| chat.id).collect();
                let selected = self.state.read(cx).selected_chat.clone();
                if let Some(target) =
                    tabs::cycle_target(&order, selected.as_deref(), command == Command::NextTab)
                {
                    self.activate_main_tab(None, cx);
                    self.open_chat(target, cx);
                }
            }
            Command::SelectTab1
            | Command::SelectTab2
            | Command::SelectTab3
            | Command::SelectTab4
            | Command::SelectTab5
            | Command::SelectTab6
            | Command::SelectTab7
            | Command::SelectTab8
            | Command::SelectLastTab => {
                let Some((_, rows, _)) = self.thread_conversation_tabs(cx) else {
                    return;
                };
                let index = match command {
                    Command::SelectTab1 => 0,
                    Command::SelectTab2 => 1,
                    Command::SelectTab3 => 2,
                    Command::SelectTab4 => 3,
                    Command::SelectTab5 => 4,
                    Command::SelectTab6 => 5,
                    Command::SelectTab7 => 6,
                    Command::SelectTab8 => 7,
                    _ => rows.len().saturating_sub(1),
                };
                if let Some(chat) = rows.get(index) {
                    let id = chat.id.clone();
                    self.activate_main_tab(None, cx);
                    self.open_chat(id, cx);
                }
            }
            Command::NextPanel | Command::PrevPanel => {
                if !matches!(self.route, Route::Chat | Route::PullRequests) {
                    return;
                }
                if matches!(self.route, Route::Chat) && self.right_surface_rows(cx).is_empty() {
                    return;
                }
                if !self.right_pane_open(cx) {
                    self.toggle_right_pane(cx);
                }
                window.focus(&self.navigation_focus.right, cx);
                self.cycle_navigation(command == Command::NextPanel, window, cx);
            }
            Command::FocusNextPane | Command::FocusPrevPane => {
                let mut panes = Vec::new();
                if self.sidebar_now() > 1. {
                    panes.push(0);
                }
                if matches!(self.route, Route::Chat) {
                    panes.push(1);
                    if self.active_main_tab(cx).is_none() {
                        panes.push(2);
                    }
                    if self.right_pane_open(cx) {
                        panes.push(3);
                    }
                }
                if panes.is_empty() {
                    return;
                }
                let current = if self.workspace_sidebar_focus.contains_focused(window, cx) {
                    0
                } else if self.navigation_focus.in_right(window, cx) {
                    3
                } else if self.composer.focus_handle(cx).contains_focused(window, cx) {
                    2
                } else if self.navigation_focus.main.contains_focused(window, cx) {
                    1
                } else {
                    4
                };
                let position = panes.iter().position(|pane| *pane == current);
                let forward = command == Command::FocusNextPane;
                let next = match (position, forward) {
                    (Some(at), true) => (at + 1) % panes.len(),
                    (Some(at), false) => (at + panes.len() - 1) % panes.len(),
                    (None, true) => 0,
                    (None, false) => panes.len() - 1,
                };
                self.composer
                    .update(cx, |composer, _| composer.focus_pending = false);
                match panes[next] {
                    0 => window.focus(&self.workspace_sidebar_focus, cx),
                    1 => window.focus(&self.navigation_focus.main, cx),
                    2 => self.focus_composer(cx),
                    _ => self.activate_right_surface(self.resolved_right_active(cx), window, cx),
                }
            }
            Command::OpenChatSwitcher | Command::SearchFiles => {
                self.toggle_command_palette(window, cx);
                let tab = if command == Command::SearchFiles {
                    command_palette::Tab::Files
                } else {
                    command_palette::Tab::Threads
                };
                self.select_palette_tab(tab, cx);
            }
            Command::ComposerTogglePlanMode => {
                self.composer
                    .update(cx, |composer, cx| composer.toggle_plan_mode(cx));
            }
            Command::ShowShortcuts => self.toggle_shortcuts_panel(cx),
            Command::FindInChat => {
                if matches!(self.route, Route::Chat) {
                    self.shortcut_conversation(window, cx)
                        .1
                        .update(cx, |transcript, cx| transcript.open_find(window, cx));
                }
            }
            Command::OpenModelPicker => window.dispatch_action(Box::new(OpenModelPicker), cx),
            Command::CycleReasoningEffort | Command::ToggleFastMode => {
                if matches!(self.route, Route::Chat) {
                    let composer = self.shortcut_conversation(window, cx).2;
                    let pickers = composer.read(cx).pickers().clone();
                    pickers.update(cx, |pickers, cx| {
                        if command == Command::CycleReasoningEffort {
                            pickers.cycle_reasoning_effort(cx)
                        } else {
                            pickers.toggle_fast_mode(cx)
                        }
                    });
                }
            }
            Command::ToggleToolDetails => {
                if matches!(self.route, Route::Chat) {
                    self.shortcut_conversation(window, cx)
                        .1
                        .update(cx, |transcript, cx| transcript.toggle_all_tool_details(cx));
                }
            }
            Command::ToggleTaskList => {
                if matches!(self.route, Route::Chat) {
                    self.shortcut_conversation(window, cx)
                        .2
                        .update(cx, |composer, cx| composer.toggle_task_list(cx));
                }
            }
            Command::StopBackgroundTasks => self.stop_background_tasks_shortcut(window, cx),
            Command::EditorAnnotate => {
                let file = self
                    .active_main_file(cx)
                    .and_then(|id| self.file_surfaces.get(&id).cloned())
                    .or_else(|| match self.resolved_right_active(cx) {
                        RightSurface::File(id) => self.file_surfaces.get(&id).cloned(),
                        _ => None,
                    });
                if let Some(file) = file {
                    file.update(cx, |file, cx| file.annotate_selection(window, cx));
                }
            }
            // These are bound directly to the focused input's action.
            Command::ComposerSubmit
            | Command::ComposerNewline
            | Command::ComposerForceSubmit
            | Command::ComposerPreviousMessage
            | Command::ComposerNextMessage
            | Command::ComposerQueueSubmit
            | Command::ComposerDeleteToStart
            | Command::ComposerDeleteToEnd
            | Command::ComposerDeleteWord
            | Command::ComposerInterrupt
            | Command::ComposerYank
            | Command::ComposerYankPop
            | Command::ComposerStash
            | Command::ComposerSearchHistory
            | Command::EditorSave => {}
        }
    }

    fn step_desktop_chat(&mut self, forward: bool, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let selected = state.selected_chat_row();
        let root = selected.map(|chat| chat.thread_id().to_owned());
        let space = selected
            .and_then(|chat| chat.space_id.clone())
            .or_else(|| state.selected_space.clone());
        let order: Vec<_> = self
            .sidebar_visible_order(cx)
            .into_iter()
            .filter(|id| {
                state
                    .chats
                    .iter()
                    .any(|chat| chat.id == *id && chat.space_id == space)
            })
            .collect();
        if let Some(target) = tabs::cycle_target(&order, root.as_deref(), forward) {
            self.open_chat(target, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    fn setup(
        cx: &mut TestAppContext,
    ) -> (Entity<Shell>, &mut VisualTestContext, tempfile::TempDir) {
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
        });
        let (shell,cx)=cx.add_window_view(|_,cx|{
            let state=cx.new(|_|AppState::new());
            let mut shell=Shell::new(state,EngineBootConfig{target:None,data_dir:dir.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock},cx);
            shell.settings.sidebar_organization=SidebarOrganization::InOneList;
            shell.state.update(cx,|state,_|{
                state.workspace_scope=Some(WorkspaceScope::Local);state.local_device_id=Some("local".into());state.no_project=true;
                state.chats=[("root",None),("tab",Some("root")),("other",None)].into_iter().enumerate().map(|(ix,(id,root))|serde_json::from_value(serde_json::json!({"id":id,"deviceId":"local","archived":false,"threadId":root,"createdAt":Utc::now()-chrono::Duration::minutes(10-ix as i64)})).unwrap()).collect();
                state.selected_chat=Some("root".into());
            });
            shell.active_chat="root".into();shell.reduced_motion=true;shell
        });
        (shell, cx, dir)
    }
    #[gpui::test]
    fn desktop_number_keys_select_thread_tabs_and_ctrl_tab_selects_chats(cx: &mut TestAppContext) {
        let (shell, cx, _dir) = setup(cx);
        cx.update(|window, cx| {
            window.draw(cx).clear();
            let focus = shell.read(cx).shortcut_focus.clone();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes(&platform_combo("mod-alt-]"));
        cx.update(|_, cx| assert!(!shell.read(cx).right_pane_open(cx)));
        cx.simulate_keystrokes(&platform_combo("mod-2"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("tab")
            )
        });
        cx.simulate_keystrokes(&platform_combo("mod-1"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("root")
            )
        });
        cx.simulate_keystrokes(&platform_combo("mod-9"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("tab")
            )
        });
        cx.simulate_keystrokes(&platform_combo("ctrl-tab"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("other")
            )
        });
    }
    #[gpui::test]
    fn imported_binding_replaces_default_and_disabled_command_stays_disabled(
        cx: &mut TestAppContext,
    ) {
        let (shell, cx, _dir) = setup(cx);
        let mut keymap = KeymapConfig::default();
        keymap.import_desktop_rules(settings::keybindings::import(&serde_json::json!({"schemaVersion":1,"rules":[{"key":"mod+shift+7","command":"select-tab-2"},{"key":"","command":"select-last-tab"}]})).unwrap());
        cx.update(|window, cx| {
            apply_keymap(cx, &keymap, ComposerSendBehavior::default());
            window.draw(cx).clear();
            let focus = shell.read(cx).shortcut_focus.clone();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes(&platform_combo("mod-2"));
        cx.simulate_keystrokes(&platform_combo("mod-9"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("root")
            )
        });
        cx.simulate_keystrokes(&platform_combo("mod-shift-7"));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).state.read(cx).selected_chat.as_deref(),
                Some("tab")
            )
        });
    }
    struct ComposerHost {
        shell: Entity<Shell>,
    }
    impl Render for ComposerHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.shell.read(cx).composer.clone())
                .on_action(cx.listener(|this, command: &DesktopCommand, window, cx| {
                    this.shell
                        .update(cx, |shell, cx| shell.desktop_command(command.0, window, cx));
                }))
        }
    }
    #[gpui::test]
    fn shift_tab_toggles_planning_only_in_the_message_composer(cx: &mut TestAppContext) {
        let (shell, _, _dir) = setup(cx);
        // Mount the real input independently of the engine boot screen.
        let (_, cx) = cx.add_window_view(|_, _| ComposerHost {
            shell: shell.clone(),
        });
        shell.update(cx, |shell, cx| {
            shell.open_new_session(None, cx);
            shell
                .composer
                .read(cx)
                .pickers()
                .clone()
                .update(cx, |pickers, cx| {
                    pickers.set_draft_sandbox(zuse_protocol::SandboxLevel::Auto, cx)
                });
        });
        cx.update(|window, cx| {
            window.draw(cx).clear();
            let focus = shell.read(cx).composer.focus_handle(cx);
            window.focus(&focus, cx);
        });
        let resolved = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                let resolved = shell
                    .read(cx)
                    .composer
                    .read(cx)
                    .pickers()
                    .read(cx)
                    .resolved(cx);
                (resolved.sandbox, resolved.plan)
            })
        };
        cx.simulate_keystrokes("shift-tab");
        // Plan mode is its own flag: the chosen access level is untouched.
        assert_eq!(resolved(cx), (zuse_protocol::SandboxLevel::Auto, true));
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(resolved(cx), (zuse_protocol::SandboxLevel::Auto, false));
    }
    #[gpui::test]
    fn chat_shortcuts_run_from_the_focused_composer(cx: &mut TestAppContext) {
        let (shell, _, _dir) = setup(cx);
        let (_, cx) = cx.add_window_view(|_, _| ComposerHost {
            shell: shell.clone(),
        });
        shell.update(cx, |shell, cx| shell.open_new_session(None, cx));
        cx.update(|window, cx| {
            window.draw(cx).clear();
            let focus = shell.read(cx).composer.focus_handle(cx);
            window.focus(&focus, cx);
        });
        let text = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                shell
                    .read(cx)
                    .composer
                    .read(cx)
                    .input
                    .read(cx)
                    .text()
                    .to_owned()
            })
        };
        // `?` in the empty composer opens the panel instead of typing.
        cx.simulate_input("?");
        cx.update(|_, cx| assert!(shell.read(cx).shortcuts_panel_open));
        assert_eq!(text(cx), "");
        cx.simulate_keystrokes(&platform_combo("mod-/"));
        cx.update(|_, cx| assert!(!shell.read(cx).shortcuts_panel_open));
        // Once there is a draft, `?` is just a character.
        cx.simulate_input("why?");
        assert_eq!(text(cx), "why?");
        cx.update(|_, cx| assert!(!shell.read(cx).shortcuts_panel_open));
        // App chords reach the transcript before the editor sees them.
        let details = Command::ToggleToolDetails.default_combo();
        cx.simulate_keystrokes(&platform_combo(details));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).transcript.read(cx).bulk_tool_details(),
                Some(true)
            )
        });
        cx.simulate_keystrokes(&platform_combo(details));
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).transcript.read(cx).bulk_tool_details(),
                Some(false)
            )
        });
        assert_eq!(text(cx), "why?");
    }
    #[test]
    fn scoped_commands_do_not_reserve_typing_chords_globally() {
        assert_eq!(
            Command::ComposerTogglePlanMode.context(),
            Some("MessageComposer")
        );
        assert_eq!(Command::EditorAnnotate.context(), Some("Input"));
        assert_eq!(Command::NextChat.context(), None);
        let prefs = UiSettings::default().clamped();
        assert_eq!(
            prefs
                .keymap
                .get(ShortcutId::Desktop(Command::ComposerForceSubmit)),
            "mod-enter"
        );
        assert!(settings::conflicted_shortcuts(&prefs.keymap).is_empty());
    }
}
