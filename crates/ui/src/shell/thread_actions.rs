use super::*;
use zuse_viewport::input::{Submitted, TextInput};

pub(super) struct CloudThreadRename {
    target: crate::state::CloudTarget,
    input: Entity<TextInput>,
    _submit: Subscription,
}

/// Width of a sidebar thread's "…" menu.
pub(super) const SIDEBAR_ACTIONS_MENU_WIDTH: f32 = 176.0;
/// Space between the "…" button and the menu below it.
const SIDEBAR_ACTIONS_MENU_OFFSET: f32 = 4.0;

impl Shell {
    /// Opens the actions menu under the "…" button, right edges aligned;
    /// falls back to the pointer before the button has been drawn.
    pub(super) fn sidebar_actions_menu_position(&self, pointer: Point<Pixels>) -> Point<Pixels> {
        let (right, bottom) = self
            .sidebar_more_anchor
            .get()
            .filter(|bounds| bounds.contains(&pointer))
            .map_or((pointer.x, pointer.y), |bounds| {
                (bounds.right(), bounds.bottom())
            });
        gpui::point(
            (right - px(SIDEBAR_ACTIONS_MENU_WIDTH)).max(px(8.)),
            bottom + px(SIDEBAR_ACTIONS_MENU_OFFSET),
        )
    }

    pub(super) fn render_sidebar_thread_actions(
        &self,
        id: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let actions = [
            ("share", "Share", icons::BROWSER_LINK),
            ("new-tab", "Open in new tab", icons::ARROW_UP_RIGHT),
            ("rename", "Rename", icons::PEN),
            ("archive", "Move to archive", icons::ARCHIVE_MINIMALISTIC),
            ("delete", "Delete", icons::TRASH_BIN_MINIMALISTIC),
        ];
        let mut rows = Vec::with_capacity(actions.len() + 1);
        let archive_available = !id.starts_with(CLOUD_ROW_PREFIX)
            || self.state.read(cx).cloud_organization_id().is_none();
        for (action, label, glyph) in actions {
            if action == "archive" && !archive_available {
                continue;
            }
            let destructive = action == "delete";
            if destructive {
                rows.push(popover::menu_separator().into_any_element());
            }
            let id = id.to_owned();
            let key = SharedString::from(format!("chat-menu-{action}"));
            rows.push(
                popover::menu_row(theme, false, key.clone())
                    .id(key.clone())
                    .debug_selector(move || key.to_string())
                    .role(gpui::Role::MenuItem)
                    // Compact rows: the sidebar menu is a quick list, not a dialog.
                    .py(px(4.))
                    .gap(px(8.))
                    .text_size(crate::typography::text_xs())
                    .when(destructive, |row| row.text_color(theme.danger))
                    .child(icon(glyph).size(px(14.)).text_color(if destructive {
                        theme.danger
                    } else {
                        theme.text_muted
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_chat_menu(cx);
                        match action {
                            "share" => this.copy_zuse_conversation_link(&id, cx),
                            "new-tab" => this.open_sidebar_thread_tab(id.clone(), cx),
                            "rename" => {
                                if let Some(workspace) = id.strip_prefix(CLOUD_ROW_PREFIX) {
                                    this.open_cloud_thread_rename(workspace, window, cx);
                                } else {
                                    this.open_rename_chat(id.clone(), cx);
                                }
                            }
                            "archive" => this.set_chat_archived(id.clone(), true, cx),
                            "delete" => {
                                this.delete_confirm = Some(id.clone());
                                cx.notify();
                            }
                            _ => unreachable!(),
                        }
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    pub(super) fn current_sidebar_thread(&self, cx: &App) -> Option<String> {
        if let Some(view) = &self.cloud_conversation {
            return Some(format!(
                "{CLOUD_ROW_PREFIX}{}",
                view.read(cx).target.workspace
            ));
        }
        self.state
            .read(cx)
            .selected_chat_row()
            .map(|chat| chat.thread_id().to_owned())
    }

    fn select_sidebar_thread(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(workspace) = id.strip_prefix(CLOUD_ROW_PREFIX) {
            // Preserve an already open conversation when returning to a tab.
            if self.current_sidebar_thread(cx).as_deref() == Some(id.as_str()) {
                self.activate_main_tab(None, cx);
            } else {
                self.open_cloud_sidebar_row(&format!("{CLOUD_ROW_PREFIX}{workspace}"), cx);
            }
        } else {
            self.open_chat(id, cx);
        }
    }

    fn open_sidebar_thread_tab(&mut self, id: String, cx: &mut Context<Self>) {
        for tab in self
            .current_sidebar_thread(cx)
            .into_iter()
            .chain([id.clone()])
        {
            if !self.sidebar_thread_tabs.contains(&tab) {
                self.sidebar_thread_tabs.push(tab);
            }
        }
        self.select_sidebar_thread(id, cx);
    }

    fn sidebar_thread_tab_title<'a>(
        &'a self,
        id: &str,
        cx: &'a App,
    ) -> Option<(&'a str, &'static str)> {
        let state = self.state.read(cx);
        if let Some(workspace) = id.strip_prefix(CLOUD_ROW_PREFIX) {
            let row = state.cloud_sidebar_workspace(workspace)?;
            Some((
                row["title"].as_str().unwrap_or("Cloud chat"),
                icons::DITHER_CLOUD,
            ))
        } else {
            let chat = state
                .chats
                .iter()
                .find(|chat| chat.id == id && !chat.archived)?;
            Some((
                chat.title.as_deref().unwrap_or("New session"),
                icons::CHAT_ROUND_LINE,
            ))
        }
    }

    pub(super) fn can_close_sidebar_thread_tab(&self, id: &str, cx: &App) -> bool {
        self.sidebar_thread_tabs.iter().any(|tab| tab == id)
            && self
                .sidebar_thread_tabs
                .iter()
                .any(|tab| tab != id && self.sidebar_thread_tab_title(tab, cx).is_some())
    }

    pub(super) fn close_sidebar_thread_tab(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let active = self.current_sidebar_thread(cx).as_deref() == Some(id);
        let Some(index) = self.sidebar_thread_tabs.iter().position(|tab| tab == id) else {
            return false;
        };
        let next = active
            .then(|| {
                let (before, after) = self.sidebar_thread_tabs.split_at(index);
                after
                    .iter()
                    .skip(1)
                    .chain(before.iter().rev())
                    .find(|tab| self.sidebar_thread_tab_title(tab, cx).is_some())
                    .cloned()
            })
            .flatten();
        self.sidebar_thread_tabs.remove(index);
        if let Some(next) = next {
            self.select_sidebar_thread(next, cx);
        }
        cx.notify();
        !active || self.current_sidebar_thread(cx).as_deref() != Some(id)
    }

    pub(super) fn sidebar_thread_tab_chips(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let active = self.current_sidebar_thread(cx);
        self.sidebar_thread_tabs
            .iter()
            .filter(|id| Some(id.as_str()) != active.as_deref())
            .filter_map(|id| {
                let (title, glyph) = self.sidebar_thread_tab_title(id, cx)?;
                let key = SharedString::from(format!("sidebar-thread-tab-{id}"));
                let open = id.clone();
                let close = id.clone();
                Some(
                    tabs::conversation_tab_chip(key.clone(), title.to_owned().into(), false, theme)
                        .debug_selector(move || key.to_string())
                        .child(
                            icon(glyph)
                                .size(px(14.))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title.to_owned()))
                        .child(
                            div()
                                .id("close-sidebar-thread-tab")
                                .role(gpui::Role::Button)
                                .aria_label("Close tab")
                                .cursor_pointer()
                                .child(
                                    icon(icons::CLOSE)
                                        .size(px(12.))
                                        .text_color(theme.text_muted),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_sidebar_thread_tab(&close, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_sidebar_thread(open.clone(), cx)
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn sidebar_cloud_thread(
        &self,
        workspace: &str,
        cx: &App,
    ) -> Option<(crate::state::CloudTarget, serde_json::Value)> {
        let state = self.state.read(cx);
        let row = state.cloud_sidebar_workspace(workspace)?.clone();
        Some((self.cloud_row_target(&row, cx)?, row))
    }

    fn open_cloud_thread_rename(
        &mut self,
        workspace: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target, row)) = self.sidebar_cloud_thread(workspace, cx) else {
            return;
        };
        let title = row["title"].as_str().unwrap_or_default().to_owned();
        let input = cx.new(|cx| {
            let mut input = TextInput::new("Chat name", window, cx);
            input.set_value(title, cx);
            input
        });
        window.focus(&input.focus_handle(cx), cx);
        let submit = cx.subscribe(&input, |this, _, _: &Submitted, cx| {
            this.submit_cloud_thread_rename(cx);
        });
        self.cloud_thread_rename = Some(CloudThreadRename {
            target,
            input,
            _submit: submit,
        });
        cx.notify();
    }

    fn submit_cloud_thread_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.cloud_thread_rename else {
            return;
        };
        let title = dialog.input.read(cx).value(cx).trim().to_owned();
        if title.is_empty() {
            return;
        }
        let target = dialog.target.clone();
        if title.chars().count() > 200 || title.chars().any(char::is_control) {
            self.show_notice(
                crate::toast::ToastKind::Info,
                "Enter a single-line name of up to 200 characters.",
                cx,
            );
            return;
        }
        self.cloud_thread_rename = None;
        cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(
                    methods::CLOUD_WORKSPACE_RENAME,
                    serde_json::json!({
                        "workspaceId": target.workspace,
                        "expectedAccountId": target.subject,
                        "expectedOrganizationId": target.organization,
                        "title": title,
                    }),
                )
                .await
                .and_then(|value| {
                    if value["accountId"] == target.subject
                        && value["organizationId"].as_str() == target.organization.as_deref()
                        && value["workspace"]["workspaceId"] == target.workspace
                        && value["workspace"]["renamedTitle"] == title
                    {
                        Ok(value)
                    } else {
                        Err(zuse_rpc::RpcError::Failed(
                            "The new name could not be verified.".into(),
                        ))
                    }
                });
            this.update(cx, |shell, cx| {
                match result {
                    Ok(value) => shell.state.update(cx, |state, cx| {
                        state.set_cloud_sidebar_title(&target, &value["workspace"], cx);
                    }),
                    Err(error) => shell.show_notice(
                        crate::toast::ToastKind::Error,
                        format!("Couldn’t rename this cloud chat. {error}"),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_cloud_thread_rename(
        &self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.cloud_thread_rename.as_ref()?;
        let theme = Theme::of(cx).for_popup();
        let card = popover::dialog_card(&theme)
            .id("rename-cloud-thread")
            .child(popover::dialog_title(&theme, "Rename chat"))
            .child(popover::dialog_field(
                dialog.input.clone().into_any_element(),
            ))
            .child(
                popover::dialog_footer(&theme)
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "rename-cloud-cancel")
                            .id("rename-cloud-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cloud_thread_rename = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(&theme, "Rename")
                            .id("rename-cloud-confirm")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.submit_cloud_thread_rename(cx)),
                            ),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-cloud-thread-dialog", viewport, card))
    }

    pub(super) fn delete_cloud_sidebar_thread(&mut self, workspace: &str, cx: &mut Context<Self>) {
        if self.deleting_chat {
            return;
        }
        let Some((target, _)) = self.sidebar_cloud_thread(workspace, cx) else {
            return;
        };
        let id = format!("{CLOUD_ROW_PREFIX}{workspace}");
        let command = self
            .cloud_delete_commands
            .entry(id.clone())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.deleting_chat = true;
        cx.spawn(async move |this, cx| {
            let result = target
                .delete_session(&target.initial_session, &command)
                .await;
            this.update(cx, |shell, cx| {
                shell.deleting_chat = false;
                match result {
                    Ok(_) => {
                        shell.cloud_delete_commands.remove(&id);
                        shell.sidebar_thread_tabs.retain(|tab| tab != &id);
                        if shell.delete_confirm.as_deref() == Some(id.as_str()) {
                            shell.delete_confirm = None;
                        }
                        shell.state.update(cx, |state, cx| {
                            if state.cloud_user_id() == Some(target.subject.as_str())
                                && state.cloud_organization_id() == target.organization.as_deref()
                            {
                                state.begin_cloud_sidebar_archive(&target.workspace);
                                state.acknowledge_cloud_sidebar_archive(&target.workspace, cx);
                                cx.notify();
                            }
                        });
                        if shell.cloud_conversation.as_ref().is_some_and(|view| {
                            let view = view.read(cx);
                            view.target.workspace == target.workspace
                                && view.target.subject == target.subject
                                && view.target.organization == target.organization
                        }) {
                            shell.open_new_session(None, cx);
                        }
                    }
                    Err(error) => shell.show_notice(
                        crate::toast::ToastKind::Error,
                        format!("Couldn’t delete this cloud chat. {error}"),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
