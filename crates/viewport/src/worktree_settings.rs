//! Worktree storage is configured on the connected engine, including paired hosts.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, TEXT, column, row},
};
use zuse_protocol::{FolderListing, WorktreeSettings, WorktreeSettingsStatus};

struct Saving {
    id: u64,
    input: String,
    enabled: Option<bool>,
}
pub(super) struct WorktreeSettingsView {
    status: Option<WorktreeSettingsStatus>,
    confirmed: bool,
    read: Option<u64>,
    save: Option<Saving>,
    path: Entity<input::TextInput>,
    enabled: Option<bool>,
    browsing: bool,
    folders: Option<FolderListing>,
    folder_request: Option<u64>,
    error: Option<String>,
}
impl WorktreeSettingsView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        let path =
            cx.new(|cx| input::TextInput::new("Absolute folder path on the engine", window, cx));
        cx.observe(&path, |_, _, cx| cx.notify()).detach();
        Self {
            status: None,
            confirmed: false,
            read: None,
            save: None,
            path,
            enabled: None,
            browsing: false,
            folders: None,
            folder_request: None,
            error: None,
        }
    }
    fn editable(&self) -> bool {
        self.confirmed
            && self.save.is_none()
            && self
                .status
                .as_ref()
                .is_some_and(|status| status.environment_override.is_none())
    }
    fn enabled(&self) -> bool {
        self.enabled.unwrap_or_else(|| {
            self.status
                .as_ref()
                .is_some_and(|status| status.settings.use_custom_directory)
        })
    }
}
impl WorkspaceView {
    pub(super) fn refresh_worktree_settings(&mut self) {
        if !self.connected
            || self.worktree_settings.read.is_some()
            || self.worktree_settings.save.is_some()
        {
            return;
        }
        let id = self.call(methods::GET_WORKTREE_SETTINGS, json!({}));
        self.worktree_settings.read = Some(id);
    }
    pub(super) fn interrupt_worktree_settings(&mut self) {
        let state = &mut self.worktree_settings;
        if state.save.take().is_some() {
            state.error = Some("The save may have completed before disconnection. The current location will be read after reconnecting. Your entered path is kept.".into());
        }
        state.read = None;
        state.folder_request = None;
        state.confirmed = false;
    }
    fn browse_worktree_folder(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if !self.connected || !self.worktree_settings.editable() {
            return;
        }
        if let Some(id) = self.worktree_settings.folder_request.take() {
            self.cancel(id);
        }
        self.worktree_settings.browsing = true;
        self.worktree_settings.folders = None;
        let id = self.call(methods::LIST_FOLDERS, json!({"path":path}));
        self.worktree_settings.folder_request = Some(id);
        cx.notify();
    }
    fn save_worktree_location(&mut self, reset: bool, cx: &mut Context<Self>) {
        if !self.require_connection(cx) || !self.worktree_settings.editable() {
            return;
        }
        let input = self.worktree_settings.path.read(cx).value(cx);
        let enabled = self.worktree_settings.enabled;
        let settings = if reset {
            WorktreeSettings::default()
        } else {
            let state = &self.worktree_settings;
            let custom_directory = if input.trim().is_empty() {
                state
                    .status
                    .as_ref()
                    .and_then(|status| status.settings.custom_directory.clone())
            } else {
                Some(input.trim().to_owned())
            };
            if state.enabled() && custom_directory.is_none() {
                self.worktree_settings.error =
                    Some("Choose a folder or enter an absolute path first.".into());
                cx.notify();
                return;
            }
            WorktreeSettings {
                use_custom_directory: state.enabled(),
                custom_directory,
            }
        };
        if let Some(id) = self.worktree_settings.read.take() {
            self.cancel(id);
        }
        self.worktree_settings.error = None;
        let id = self.call(
            methods::SET_WORKTREE_SETTINGS,
            serde_json::to_value(settings).unwrap(),
        );
        self.worktree_settings.save = Some(Saving { id, input, enabled });
        cx.notify();
    }
    pub(super) fn receive_worktree_settings(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let read = self.worktree_settings.read == Some(id);
        let save = self
            .worktree_settings
            .save
            .as_ref()
            .is_some_and(|save| save.id == id);
        let folders = self.worktree_settings.folder_request == Some(id);
        if !read && !save && !folders {
            return false;
        }
        let value = frame.get("ok").or_else(|| frame.get("item"));
        if value.is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.pending.remove(&id);
        let result = if let Some(value) = value {
            if folders {
                serde_json::from_value::<FolderListing>(value.clone())
                    .map(|listing| {
                        self.worktree_settings.folders = Some(listing);
                    })
                    .map_err(|_| "The engine returned an invalid folder listing.".to_owned())
            } else {
                serde_json::from_value::<WorktreeSettingsStatus>(value.clone())
                    .map(|status| {
                        self.worktree_settings.status = Some(status);
                        self.worktree_settings.confirmed = true;
                    })
                    .map_err(|_| {
                        "The engine returned invalid worktree settings. Refresh before saving."
                            .to_owned()
                    })
            }
        } else {
            Err(frame["err"]
                .as_str()
                .unwrap_or("The request could not be confirmed. Refresh before trying again.")
                .to_owned())
        };
        let success = result.is_ok();
        if let Err(error) = result {
            self.worktree_settings.error = Some(error);
        } else if !read {
            self.worktree_settings.error = None;
        }
        if read {
            self.worktree_settings.read = None;
        }
        if folders {
            self.worktree_settings.folder_request = None;
        }
        if save {
            let save = self.worktree_settings.save.take().unwrap();
            if success {
                if self.worktree_settings.path.read(cx).value(cx) == save.input {
                    self.worktree_settings
                        .path
                        .update(cx, |input, cx| input.set_value(String::new(), cx));
                }
                if self.worktree_settings.enabled == save.enabled {
                    self.worktree_settings.enabled = None;
                }
                self.worktree_settings.browsing = false;
                if let Some(id) = self.worktree_settings.folder_request.take() {
                    self.cancel(id);
                }
            } else {
                // A missing receipt is not a reason to repeat a filesystem mutation.
                self.worktree_settings.confirmed = false;
                self.refresh_worktree_settings();
            }
        }
        cx.notify();
        true
    }
    pub(super) fn worktree_settings_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.worktree_settings;
        let editable = self.connected && state.editable();
        let saving = state.save.is_some();
        let title = if self.paired {
            "Worktrees on paired computer"
        } else {
            "Worktrees"
        };
        let location = state
            .status
            .as_ref()
            .map(|status| status.effective_directory.clone());
        let override_path = state
            .status
            .as_ref()
            .and_then(|status| status.environment_override.clone());
        let body = s::card()
            .child(s::setting_row(true)
                .child(s::details("Custom worktree location", "Choose where new worktrees are created on this computer. Existing threads keep their current folders."))
                .child(s::toggle("custom-worktree-location", state.enabled(), "Custom worktree location")
                    .opacity(if editable { 1. } else { 0.45 })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if editable { this.worktree_settings.enabled = Some(!this.worktree_settings.enabled()); cx.notify(); }
                    }))))
            .when_some(location, |card, path| card.child(s::setting_row(false).flex_col().items_start()
                .child(s::title_text(if state.confirmed { "Current location" } else { "Last known location" }))
                .child(s::description_text(path).overflow_hidden().text_ellipsis())))
            .when_some(override_path.clone(), |card, path| card.child(s::setting_row(false)
                .child(s::details("Managed by this computer", format!("ZUSE_WORKTREES_DIR sets the location to {path}. Change that setting on the engine to choose another folder.")))))
            .when(state.enabled() && override_path.is_none(), |card| card.child(s::setting_row(false).flex_col().items_stretch()
                .child(s::field().w_full().child(state.path.clone()))
                .when_some(state.status.as_ref().and_then(|status| status.settings.custom_directory.clone()), |view, path| view.child(s::description_text(format!("Saved folder: {path}"))))
                .child(row().gap_2().flex_wrap()
                    .child(s::button("browse-worktree-folder", "Browse folders").opacity(if editable { 1. } else { 0.45 })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !editable { return; }
                            let entered = this.worktree_settings.path.read(cx).value(cx);
                            let path = if entered.trim().is_empty() { this.worktree_settings.status.as_ref().and_then(|status| status.settings.custom_directory.clone()) } else { Some(entered.trim().to_owned()) };
                            this.browse_worktree_folder(path, cx);
                        })))
                    .child(s::button("browse-worktree-home", "Home").opacity(if editable { 1. } else { 0.45 })
                        .on_click(cx.listener(move |this, _, _, cx| { if editable { this.browse_worktree_folder(None, cx); } }))))))
            .when(state.browsing, |card| card.child(s::setting_row(false).flex_col().items_stretch().child(self.worktree_folder_browser(cx))))
            .child(s::setting_row(false).items_center().justify_end()
                .child(s::button("refresh-worktree-settings", if state.read.is_some() { "Refreshing…" } else { "Refresh" })
                    .on_click(cx.listener(|this, _, _, cx| { this.refresh_worktree_settings(); cx.notify(); })))
                .child(s::button("reset-worktree-location", "Use default").opacity(if editable { 1. } else { 0.45 })
                    .on_click(cx.listener(move |this, _, _, cx| { if editable { this.save_worktree_location(true, cx); } })))
                .child(s::button("save-worktree-location", if saving { "Saving…" } else { "Save location" }).opacity(if editable { 1. } else { 0.45 })
                    .on_click(cx.listener(move |this, _, _, cx| { if editable { this.save_worktree_location(false, cx); } }))));
        s::frame()
            .child(s::header(title, "Local storage for new agent worktrees."))
            .child(body)
            .when(!self.connected, |view| {
                view.child(
                    s::description_text(
                        "Reconnect to read or change this computer’s worktree location.",
                    )
                    .px_3()
                    .py_2(),
                )
            })
            .when_some(state.error.clone(), |view, error| {
                view.child(
                    s::description_text(error)
                        .text_color(rgb(0xec9292))
                        .px_3()
                        .py_2(),
                )
            })
            .into_any_element()
    }
    fn worktree_folder_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.worktree_settings;
        let editable = self.connected && state.editable();
        let current = state.folders.as_ref().map(|listing| listing.path.clone());
        column()
            .gap_2()
            .child(
                row()
                    .gap_2()
                    .child(s::title_text("Choose a folder").flex_1())
                    .child(
                        s::button("close-worktree-browser", "Close").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.worktree_settings.browsing = false;
                                if let Some(id) = this.worktree_settings.folder_request.take() {
                                    this.cancel(id);
                                }
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(state.folder_request.is_some(), |view| {
                view.child(s::description_text("Loading folders…"))
            })
            .when_some(current, |view, path| {
                let select_path = path.clone();
                let parent = zuse_protocol::device_paths::parent_folder(&path);
                view.child(
                    s::description_text(path.clone())
                        .overflow_hidden()
                        .text_ellipsis(),
                )
                .child(
                    row()
                        .gap_2()
                        .flex_wrap()
                        .when_some(parent, |view, parent| {
                            view.child(s::button("worktree-folder-up", "Up").on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.browse_worktree_folder(Some(parent.clone()), cx)
                                },
                            )))
                        })
                        .child(
                            s::button("select-worktree-folder", "Use this folder")
                                .opacity(if editable { 1. } else { 0.45 })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !editable {
                                        return;
                                    }
                                    this.worktree_settings.path.update(cx, |input, cx| {
                                        input.set_value(select_path.clone(), cx)
                                    });
                                    this.worktree_settings.enabled = Some(true);
                                    this.worktree_settings.browsing = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    column()
                        .id("worktree-folder-list")
                        .max_h(px(224.))
                        .overflow_y_scroll()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .rounded(px(7.))
                        .children(
                            state
                                .folders
                                .as_ref()
                                .into_iter()
                                .flat_map(|listing| listing.entries.iter())
                                .filter(|entry| entry.is_dir)
                                .enumerate()
                                .map(|(index, entry)| {
                                    let child = zuse_protocol::device_paths::child_folder(
                                        &path,
                                        &entry.name,
                                    );
                                    row()
                                        .id(("worktree-folder", index))
                                        .role(Role::Button)
                                        .aria_label(format!("Open {}", entry.name))
                                        .tab_index(0)
                                        .cursor_pointer()
                                        .px_3()
                                        .py_2()
                                        .gap_2()
                                        .hover(|view| view.bg(s::frame_bg()))
                                        .child(icon("folder").size(px(14.)).text_color(rgb(MUTED)))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_size(px(12.))
                                                .text_color(rgb(TEXT))
                                                .text_ellipsis()
                                                .child(entry.name.clone()),
                                        )
                                        .child(icon("alt-arrow-right").size(px(12.)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.browse_worktree_folder(Some(child.clone()), cx)
                                        }))
                                }),
                        )
                        .when(
                            state.folders.as_ref().is_some_and(|listing| {
                                !listing.entries.iter().any(|entry| entry.is_dir)
                            }),
                            |view| view.child(s::description_text("No subfolders").p_3()),
                        ),
                )
            })
            .when(
                state
                    .folders
                    .as_ref()
                    .is_some_and(|listing| listing.truncated),
                |view| {
                    view.child(s::description_text(
                        "This folder has more entries. Enter a specific path to open it.",
                    ))
                },
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn parity_worktree_save_retires_old_reads_and_reconciles_without_replay(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                let status = WorktreeSettingsStatus {
                    settings: WorktreeSettings {
                        use_custom_directory: true,
                        custom_directory: Some("/disk/worktrees".into()),
                    },
                    default_directory: "/home/user/.zuse/worktrees".into(),
                    effective_directory: "/disk/worktrees".into(),
                    environment_override: None,
                };
                view.refresh_worktree_settings();
                let read = view.worktree_settings.read.unwrap();
                view.receive(json!({"id":read,"ok":status}), cx);
                assert!(view.worktree_settings.editable());
                view.refresh_worktree_settings();
                let stale = view.worktree_settings.read.unwrap();
                view.worktree_settings.enabled = Some(false);
                view.save_worktree_location(false, cx);
                let save = view.worktree_settings.save.as_ref().unwrap().id;
                view.receive(json!({"id":stale,"ok":{"bad":"old read"}}), cx);
                assert!(view.worktree_settings.confirmed);
                view.receive(json!({"id":save,"err":"Lost receipt","done":true}), cx);
                assert!(view.worktree_settings.save.is_none());
                assert!(!view.worktree_settings.confirmed);
                let refresh = view.worktree_settings.read.unwrap();
                let mut reconciled = status.clone();
                reconciled.settings.use_custom_directory = false;
                reconciled.effective_directory = reconciled.default_directory.clone();
                view.receive(json!({"id":refresh,"ok":reconciled}), cx);
                assert!(view.worktree_settings.confirmed);
                assert_eq!(view.worktree_settings.enabled, Some(false));
                assert_eq!(
                    view.worktree_settings
                        .status
                        .as_ref()
                        .unwrap()
                        .settings
                        .custom_directory
                        .as_deref(),
                    Some("/disk/worktrees")
                );
                view.worktree_settings
                    .status
                    .as_mut()
                    .unwrap()
                    .environment_override = Some("/managed".into());
                view.save_worktree_location(true, cx);
                assert!(view.worktree_settings.save.is_none());
            })
            .unwrap();
        let mut writes = vec![];
        while let Ok(request) = sent.try_recv() {
            if request["method"] == methods::SET_WORKTREE_SETTINGS {
                writes.push(request);
            }
        }
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["params"]["useCustomDirectory"], false);
        assert_eq!(writes[0]["params"]["customDirectory"], "/disk/worktrees");
        assert!(writes[0]["params"].get("targetDeviceId").is_none());
    }
}
