//! Shared native/browser Git panel. The host supplies transport; actions never replay.
use crate::plugin_setup::SetupColors;
use gpui::{prelude::*, *};
use gpui_base::input::{InputEditorStyle, InputState};
use serde_json::{Value, json};
use zuse_protocol::{
    GitFileState,
    git_workspace::{GitOperation, GitWorkspace},
    methods,
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub chat_id: String,
    pub cwd: String,
    pub device_id: String,
    pub archived: bool,
}
#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub method: &'static str,
    pub params: Value,
}
pub struct GitControls {
    target: Option<Target>,
    snapshot: Option<GitWorkspace>,
    next: u64,
    pending: Option<(u64, bool)>,
    error: Option<String>,
    expanded: bool,
    clear_inputs: bool,
    notice: Option<String>,
    pending_commit: bool,
    message: Entity<InputState>,
    branch: Entity<InputState>,
    scroll: UniformListScrollHandle,
    pub colors: SetupColors,
    applied_colors: SetupColors,
    _inputs: Vec<Subscription>,
}
impl EventEmitter<Request> for GitControls {}
impl GitControls {
    pub fn new(colors: SetupColors, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = InputEditorStyle {
            foreground: colors.text.into(),
            muted_foreground: colors.muted.into(),
            background: colors.background.into(),
            border: colors.border.into(),
            caret: colors.text.into(),
            ..Default::default()
        };
        let mut field = |placeholder: &str| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder.to_owned());
                input.set_editor_style(style.clone());
                input
            })
        };
        let message = field("Commit message");
        let branch = field("Branch name");
        let subscriptions = [&message, &branch]
            .into_iter()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect();
        Self {
            target: None,
            snapshot: None,
            next: 0,
            pending: None,
            error: None,
            expanded: false,
            clear_inputs: false,
            notice: None,
            pending_commit: false,
            message,
            branch,
            scroll: Default::default(),
            colors,
            applied_colors: colors,
            _inputs: subscriptions,
        }
    }
    pub fn set_target(&mut self, target: Option<Target>, cx: &mut Context<Self>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.snapshot = None;
        self.pending = None;
        self.error = None;
        self.notice = None;
        self.clear_inputs = true;
        self.scroll = Default::default();
        if self.expanded {
            self.refresh(cx);
        }
        cx.notify();
    }
    pub fn disconnected(&mut self, cx: &mut Context<Self>) {
        if self.pending.take().is_some() {
            self.error =
                Some("Connection interrupted. Refresh Git before repeating an action.".into());
        }
        self.snapshot = None;
        cx.notify();
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.send(None, cx);
    }
    fn send(&mut self, operation: Option<GitOperation>, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(target) = self.target.as_ref().filter(|t| !t.archived) else {
            return;
        };
        let mut params =
            json!({"chatId":target.chat_id,"cwd":target.cwd,"targetDeviceId":target.device_id});
        let mutation = operation.is_some();
        self.pending_commit = matches!(operation.as_ref(), Some(GitOperation::Commit { .. }));
        if mutation {
            self.notice = None;
        }
        if let Some(operation) = operation {
            let Some(snapshot) = self.snapshot.as_ref().filter(|s| s.complete) else {
                return;
            };
            params["checkoutId"] = json!(snapshot.checkout_id);
            params["revision"] = json!(snapshot.revision);
            params["operation"] = serde_json::to_value(operation).unwrap();
        }
        self.next = self.next.wrapping_add(1);
        self.pending = Some((self.next, mutation));
        cx.emit(Request {
            id: self.next,
            method: if mutation {
                methods::GIT_WORKSPACE_ACTION
            } else {
                methods::GET_GIT_WORKSPACE
            },
            params,
        });
        cx.notify();
    }
    pub fn respond(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some((current, mutation)) = self.pending else {
            return;
        };
        if current != id {
            return;
        }
        self.pending = None;
        match result.and_then(|value| {
            serde_json::from_value::<GitWorkspace>(value)
                .map_err(|_| "Invalid Git response. Refresh before continuing.".into())
        }) {
            Ok(snapshot)
                if self
                    .target
                    .as_ref()
                    .is_some_and(|t| t.chat_id == snapshot.chat_id) =>
            {
                self.snapshot = Some(snapshot);
                self.error = None;
                if mutation {
                    self.notice = None;
                    if self.pending_commit {
                        self.clear_inputs = true;
                    }
                }
            }
            Ok(_) => {
                self.error = Some("The Git response belongs to another thread.".into());
            }
            Err(error) => {
                if mutation {
                    self.notice = Some(error);
                    self.snapshot = None;
                    self.refresh(cx);
                } else {
                    self.error = Some(error);
                }
            }
        }
        cx.notify();
    }
    fn staged(&self) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .filter(|f| {
                        !matches!(f.index, GitFileState::Unchanged | GitFileState::Untracked)
                    })
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn unstaged(&self) -> Vec<String> {
        self.snapshot
            .as_ref()
            .map(|s| {
                s.files
                    .iter()
                    .filter(|f| !matches!(f.worktree, GitFileState::Unchanged))
                    .map(|f| f.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn files(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(snapshot) = &self.snapshot else {
            return vec![];
        };
        range
            .filter_map(|i| snapshot.files.get(i).cloned().map(|file| (i, file)))
            .map(|(i, file)| {
                let path = file.path.clone();
                let unstage = file.path.clone();
                let staged = !matches!(
                    file.index,
                    GitFileState::Unchanged | GitFileState::Untracked
                );
                let work = file.worktree != GitFileState::Unchanged;
                div()
                    .id(("git-file", i))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .child(file.path),
                    )
                    .when(work, |d| {
                        d.child(self.button(("git-stage", i), "Stage").on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.send(
                                    Some(GitOperation::Stage {
                                        paths: vec![path.clone()],
                                    }),
                                    cx,
                                )
                            },
                        )))
                    })
                    .when(staged, |d| {
                        d.child(
                            self.button(("git-unstage", i), "Unstage")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.send(
                                        Some(GitOperation::Unstage {
                                            paths: vec![unstage.clone()],
                                        }),
                                        cx,
                                    )
                                })),
                        )
                    })
                    .into_any_element()
            })
            .collect()
    }
    fn button(&self, id: impl Into<ElementId>, text: &str) -> Stateful<Div> {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(5.))
            .border_1()
            .border_color(self.colors.border)
            .text_size(px(11.))
            .child(text.to_owned())
            .when(self.pending.is_some(), |d| d.opacity(0.45))
    }
}
impl Render for GitControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.applied_colors != self.colors {
            self.applied_colors = self.colors;
            let c = self.colors;
            let style = InputEditorStyle {
                foreground: c.text.into(),
                muted_foreground: c.muted.into(),
                background: c.background.into(),
                border: c.border.into(),
                caret: c.text.into(),
                ..Default::default()
            };
            for field in [&self.message, &self.branch] {
                field.update(cx, |input, _| input.set_editor_style(style.clone()));
            }
        }
        if self.clear_inputs {
            self.clear_inputs = false;
            self.message
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.branch
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        let target = self.target.as_ref();
        let archived = target.is_some_and(|t| t.archived);
        let available = target.is_some() && !archived;
        let branch = self
            .snapshot
            .as_ref()
            .map(|s| s.branch.clone())
            .unwrap_or_else(|| "Git".into());
        let staged = self.staged();
        let unstaged = self.unstaged();
        let mut view = div()
            .flex_none()
            .flex()
            .flex_col()
            .text_color(self.colors.text)
            .border_b_1()
            .border_color(self.colors.border)
            .px(px(10.))
            .py(px(6.))
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        self.button(
                            "git-panel-toggle",
                            &format!("{} {}", if self.expanded { "▾" } else { "▸" }, branch),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.expanded = !this.expanded;
                            if this.expanded && this.snapshot.is_none() {
                                this.refresh(cx);
                            }
                            cx.notify();
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        self.button(
                            "git-refresh",
                            if self.pending.is_some() {
                                "Working…"
                            } else {
                                "Refresh Git"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            );
        if !self.expanded {
            return view;
        }
        if !available {
            return view.child(
                div()
                    .text_size(px(11.))
                    .text_color(self.colors.muted)
                    .child(if archived {
                        "Unarchive this thread to use Git controls."
                    } else {
                        "Select a thread with a Git checkout."
                    }),
            );
        }
        if let Some(snapshot) = &self.snapshot {
            let counts = format!(
                "{} staged · {} unstaged · ↑{} ↓{}",
                staged.len(),
                unstaged.len(),
                snapshot.ahead,
                snapshot.behind
            );
            view = view
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(self.colors.muted)
                        .child(counts),
                )
                .when(!snapshot.complete, |d| {
                    d.child(
                        div()
                            .text_color(self.colors.danger)
                            .child("Partial status. Actions are unavailable."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            self.button("git-stage-all", "Stage all")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !unstaged.is_empty() {
                                        this.send(
                                            Some(GitOperation::Stage {
                                                paths: unstaged.clone(),
                                            }),
                                            cx,
                                        );
                                    }
                                })),
                        )
                        .child(self.button("git-unstage-all", "Unstage all").on_click(
                            cx.listener(move |this, _, _, cx| {
                                if !staged.is_empty() {
                                    this.send(
                                        Some(GitOperation::Unstage {
                                            paths: staged.clone(),
                                        }),
                                        cx,
                                    );
                                }
                            }),
                        )),
                );
            if !snapshot.files.is_empty() {
                view = view.child(
                    uniform_list(
                        "git-changed-files",
                        snapshot.files.len(),
                        cx.processor(Self::files),
                    )
                    .h(px((snapshot.files.len().min(6) * 28) as f32))
                    .track_scroll(&self.scroll),
                );
            }
            let commit = self
                .button("git-commit", "Commit staged")
                .on_click(cx.listener(|this, _, _, cx| {
                    let message = this.message.read(cx).value().to_string();
                    this.send(Some(GitOperation::Commit { message }), cx);
                }));
            let create = self
                .button("git-create-branch", "Create")
                .on_click(cx.listener(|this, _, _, cx| {
                    let name = this.branch.read(cx).value().trim().to_owned();
                    this.send(Some(GitOperation::CreateBranch { name }), cx);
                }));
            let switch = self
                .button("git-switch-branch", "Switch")
                .on_click(cx.listener(|this, _, _, cx| {
                    let name = this.branch.read(cx).value().trim().to_owned();
                    this.send(Some(GitOperation::SwitchBranch { name }), cx);
                }));
            view = view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().child(self.message.clone()))
                    .child(commit),
            );
            view = view.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(self.button("git-pull", "Pull").on_click(
                        cx.listener(|this, _, _, cx| this.send(Some(GitOperation::Pull), cx)),
                    ))
                    .child(self.button("git-push", "Push").on_click(
                        cx.listener(|this, _, _, cx| this.send(Some(GitOperation::Push), cx)),
                    )),
            );
            view = view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w_0().child(self.branch.clone()))
                    .child(create)
                    .child(switch),
            );
        }
        view.when_some(self.notice.clone().or(self.error.clone()), |d, error| {
            d.child(
                div()
                    .text_size(px(11.))
                    .text_color(self.colors.danger)
                    .child(error),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, prelude::v1::test, rc::Rc};
    fn snapshot(chat: &str) -> Value {
        json!({"chatId":chat,"checkoutId":chat,"cwd":"/repo","branch":"main","headSha":"head","upstream":null,"ahead":0,"behind":0,"files":[{"path":"file","oldPath":null,"index":"modified","worktree":"unchanged"}],"complete":true,"revision":"revision"})
    }
    #[gpui::test]
    fn panel_fences_old_thread_receipts_and_refreshes_without_replaying_mutations(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::initialize);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let captured = calls.clone();
        let window = cx.add_window(|window, cx| {
            let color: Hsla = gpui::rgb(0x202020).into();
            GitControls::new(
                SetupColors {
                    text: color,
                    muted: color,
                    background: color,
                    border: color,
                    danger: color,
                },
                window,
                cx,
            )
        });
        let panel = window.root(cx).unwrap();
        let _events = cx.update(|cx| {
            cx.subscribe(&panel, move |_, event: &Request, _| {
                captured.borrow_mut().push(event.clone())
            })
        });
        window
            .update(cx, |view, _, cx| {
                view.set_target(
                    Some(Target {
                        chat_id: "a".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: false,
                    }),
                    cx,
                );
                view.refresh(cx);
                let id = view.pending.unwrap().0;
                view.respond(id, Ok(snapshot("a")), cx);
                view.send(
                    Some(GitOperation::Stage {
                        paths: vec!["file".into()],
                    }),
                    cx,
                );
                let stale = view.pending.unwrap().0;
                view.set_target(
                    Some(Target {
                        chat_id: "b".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: false,
                    }),
                    cx,
                );
                view.respond(stale, Ok(snapshot("a")), cx);
                assert!(view.snapshot.is_none());
                view.refresh(cx);
                let id = view.pending.unwrap().0;
                view.respond(id, Ok(snapshot("b")), cx);
                view.send(
                    Some(GitOperation::Commit {
                        message: "Commit".into(),
                    }),
                    cx,
                );
                let id = view.pending.unwrap().0;
                view.respond(id, Err("Confirmation lost".into()), cx);
                assert_eq!(view.pending.unwrap().1, false);
                let read = view.pending.unwrap().0;
                view.respond(read, Ok(snapshot("b")), cx);
                assert_eq!(view.notice.as_deref(), Some("Confirmation lost"));
                view.set_target(
                    Some(Target {
                        chat_id: "b".into(),
                        cwd: "/repo".into(),
                        device_id: "host".into(),
                        archived: true,
                    }),
                    cx,
                );
                view.refresh(cx);
                assert!(view.pending.is_none());
            })
            .unwrap();
        let calls = calls.borrow();
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.method == methods::GIT_WORKSPACE_ACTION)
                .count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.method == methods::GET_GIT_WORKSPACE)
                .count(),
            3
        );
    }
}
