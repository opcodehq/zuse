//! Saved cloud history uses a separate viewport with no execution transport.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, column, row},
};
use zuse_model::cloud::{ArchivedCloud, Head, Workspace};
const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
const READ: &str = zuse_protocol::methods::CLOUD_HISTORY;
const UNARCHIVE: &str = zuse_protocol::methods::CLOUD_UNARCHIVE;
#[derive(Default)]
pub(super) struct ArchiveView {
    pub state: ArchivedCloud,
    owner: Option<String>,
    loaded: bool,
    list: Option<u64>,
    read: Option<(u64, u64)>,
    action: Option<(u64, String)>,
    view: Option<Entity<WorkspaceView>>,
    commands: HashMap<String, String>,
}
impl WorkspaceView {
    pub(super) fn interrupt_cloud_archive(&mut self) {
        self.cloud_archive.list = None;
        self.cloud_archive.read = None;
        self.cloud_archive.loaded = false;
        self.cloud_archive.state.loading = false;
        self.cloud_archive.state.busy = false;
        if self.cloud_archive.action.take().is_some() {
            self.cloud_archive.state.error = Some(
                "Connection interrupted. Refresh to check whether Unarchive completed.".into(),
            );
        }
    }

    pub(super) fn load_cloud_archive(&mut self) {
        if !self.cached_only
            && self.connected
            && self.cloud_archive.owner.is_some()
            && !self.cloud_archive.loaded
            && self.cloud_archive.list.is_none()
        {
            self.cloud_archive.list = Some(self.call(LIST, json!({})));
        }
    }
    pub(super) fn receive_cloud_archive(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if self
            .pending
            .get(&id)
            .is_some_and(|m| m == methods::AUTH_STATUS)
        {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                let owner = match serde_json::from_value::<zuse_protocol::AuthState>(value.clone())
                {
                    Ok(zuse_protocol::AuthState::SignedIn { user, .. }) => Some(user.id),
                    _ => None,
                };
                if owner != self.cloud_archive.owner {
                    let mut state = self.cloud_archive.state.clone();
                    state.clear();
                    self.cloud_archive = ArchiveView {
                        owner,
                        state,
                        ..Default::default()
                    };
                }
                cx.notify();
            }
            return true;
        }
        let list = self.cloud_archive.list == Some(id);
        let read = self.cloud_archive.read.filter(|(req, _)| *req == id);
        let action = self
            .cloud_archive
            .action
            .clone()
            .filter(|(req, _)| *req == id);
        if !list && read.is_none() && action.is_none() {
            return false;
        }
        self.pending.remove(&id);
        if list {
            self.cloud_archive.list = None;
            self.cloud_archive.loaded = true;
        }
        if read.is_some() {
            self.cloud_archive.read = None;
            self.cloud_archive.state.loading = false;
        }
        if action.is_some() {
            self.cloud_archive.action = None;
            self.cloud_archive.state.busy = false;
        }
        let value = &frame["ok"];
        let error = if frame.get("err").is_some() {
            Some(
                frame["err"]
                    .as_str()
                    .unwrap_or("Cloud request failed. Try again.")
                    .to_owned(),
            )
        } else if value["accountId"].as_str() != self.cloud_archive.owner.as_deref() {
            Some("Your account changed. Reopen this view.".into())
        } else {
            None
        };
        if let Some(error) = error {
            self.cloud_archive.state.error = Some(error);
        } else if list {
            match serde_json::from_value::<Vec<Workspace>>(value["workspaces"].clone()) {
                Ok(rows) => self
                    .cloud_archive
                    .state
                    .list(self.cloud_archive.owner.clone().unwrap(), rows),
                Err(_) => {
                    self.cloud_archive.state.error =
                        Some("Cloud list is invalid. Try again.".into())
                }
            }
        } else if let Some((_, revision)) = read {
            match serde_json::from_value::<Head>(value["head"].clone()) {
                Ok(head) => {
                    self.cloud_archive.state.receive(revision, head);
                }
                Err(_) => {
                    self.cloud_archive.state.error = Some("Saved cloud history is invalid.".into())
                }
            }
        } else if let Some((_, workspace)) = action {
            self.cloud_archive.state.restored(&workspace);
            self.cloud_archive.view = None;
            self.cloud_archive.commands.remove(&workspace);
        }
        cx.notify();
        true
    }
    fn open_cloud_archive(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.read.is_some() || self.cloud_archive.state.busy {
            return;
        }
        if let Some(revision) = self.cloud_archive.state.select(id) {
            self.cloud_archive.view = None;
            let req = self.call(READ, json!({"workspaceId":id}));
            self.cloud_archive.read = Some((req, revision));
            cx.notify();
        }
    }
    fn unarchive_cloud(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.cloud_archive.action.is_some()
            || !self
                .cloud_archive
                .state
                .rows
                .iter()
                .any(|r| r.workspace_id == id && r.state == "archived")
        {
            return;
        }
        let command = self
            .cloud_archive
            .commands
            .entry(id.into())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.cloud_archive.state.error = None;
        self.cloud_archive.state.busy = true;
        let req = self.call(UNARCHIVE, json!({"workspaceId":id,"commandId":command}));
        self.cloud_archive.action = Some((req, id.into()));
        cx.notify();
    }
    pub(super) fn cloud_archive_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<_> = self
            .cloud_archive
            .state
            .rows
            .iter()
            .filter(|w| w.archived())
            .cloned()
            .collect();
        s::frame()
            .child(s::header(
                "Cloud sessions",
                "Saved conversations remain available while compute is paused.",
            ))
            .child(
                s::card().children(rows.iter().enumerate().map(|(index, w)| {
                    let open = w.workspace_id.clone();
                    let restore = open.clone();
                    s::setting_row(index == 0)
                        .child(
                            div()
                                .id(("cloud-archive-open", index))
                                .role(Role::Button)
                                .aria_label(format!("Open {}", w.label()))
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .child(s::details(
                                    w.label().to_owned(),
                                    if w.state == "archiving" {
                                        "Archiving…"
                                    } else {
                                        "Cloud · Archived"
                                    },
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_cloud_archive(&open, cx)
                                })),
                        )
                        .child(
                            s::button(("cloud-archive-restore", index), "Unarchive")
                                .opacity(
                                    if self.cloud_archive.state.busy || w.state != "archived" {
                                        0.45
                                    } else {
                                        1.
                                    },
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.unarchive_cloud(&restore, cx)
                                })),
                        )
                })),
            )
            .when(rows.is_empty(), |view| {
                view.child(
                    s::description_text(if self.cloud_archive.list.is_some() {
                        "Loading cloud sessions…"
                    } else if self.cloud_archive.owner.is_none() {
                        "Sign in to view archived cloud sessions."
                    } else {
                        "No archived cloud sessions."
                    })
                    .p_3(),
                )
            })
            .when(self.cloud_archive.state.loading, |v| {
                v.child(s::description_text("Opening saved history…").p_3())
            })
            .when_some(self.cloud_archive.state.error.clone(), |v, error| {
                v.child(s::description_text(error).p_3())
            })
            .child(
                s::button("cloud-archive-refresh", "Refresh").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.cloud_archive.loaded = false;
                        this.cloud_archive.state.error = None;
                        this.load_cloud_archive();
                        cx.notify();
                    },
                )),
            )
            .into_any_element()
    }
    pub(super) fn cloud_archive_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let head = self.cloud_archive.state.head.as_ref().unwrap().clone();
        if self.cloud_archive.view.is_none() {
            let (requests, _discard) = futures::channel::mpsc::unbounded();
            let (_events, events) = futures::channel::mpsc::unbounded();
            let child = cx.new(|cx| {
                let mut view = Self::new(Transport { requests, events }, window, cx);
                view.install_cached_history(&head);
                view
            });
            self.cloud_archive.view = Some(child);
            window.focus(&self.settings_nav_focus, cx);
        }
        let id = head.workspace_id.clone();
        column()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .child(
                row()
                    .h(px(40.))
                    .flex_none()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        s::button("cloud-history-back", "Back").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.cloud_archive.state.back();
                                this.cloud_archive.view = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div().flex_1().child(
                            head.chat
                                .title
                                .clone()
                                .unwrap_or_else(|| "Cloud conversation".into()),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child("Cloud"),
                    ),
            )
            .child(self.cloud_archive.view.clone().unwrap())
            .child(
                row()
                    .flex_none()
                    .px_4()
                    .py_2()
                    .gap_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        icon("archive-minimalistic")
                            .size(px(16.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        column()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("This chat is archived.")
                            .when_some(self.cloud_archive.state.error.clone(), |v, error| {
                                v.child(error)
                            }),
                    )
                    .child(
                        s::button(
                            "cloud-history-unarchive",
                            if self.cloud_archive.state.busy {
                                "Unarchiving…"
                            } else {
                                "Unarchive"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.unarchive_cloud(&id, cx))),
                    ),
            )
            .into_any_element()
    }
}
