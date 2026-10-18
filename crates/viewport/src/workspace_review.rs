//! Checkout review uses snapshot checksums and lazy per-file source reads.
use super::*;
use crate::{
    assets::icon,
    shell::{column, control, label, row},
    tool_results::{ResultLine, diff_lines},
};
use std::sync::Arc;
#[derive(Clone, Default)]
pub(super) enum ReviewScope {
    #[default]
    WorkingTree,
    LatestTurn,
    Commit {
        sha: String,
        subject: String,
    },
}
impl ReviewScope {
    fn params(&self) -> Value {
        match self {
            Self::WorkingTree => json!({"mode":"workingTree"}),
            Self::LatestTurn => json!({"mode":"turn"}),
            Self::Commit { sha, .. } => json!({"mode":"commit","commitSha":sha}),
        }
    }
    fn working(&self) -> bool {
        matches!(self, Self::WorkingTree)
    }
}
#[derive(Default)]
pub(super) struct WorkspaceReview {
    request: Option<u64>,
    scroll: UniformListScrollHandle,
    path: Option<String>,
    error: Option<String>,
    lines: Arc<Vec<ResultLine>>,
    can_open: bool,
    notice: Option<String>,
}
impl WorkspaceView {
    pub(super) fn interrupt_review(&mut self) {
        self.changes_request = None;
        if self.review.request.take().is_some() {
            self.review.error = Some("Diff read interrupted. Refresh when connected.".into());
        }
    }
    pub(super) fn reset_review(&mut self) {
        if let Some(id) = self.review.request.take() {
            self.cancel(id);
        }
        self.review = WorkspaceReview::default();
    }
    pub(super) fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.changes_request.take() {
            self.cancel(id);
        }
        self.reset_review();
        self.changes_error = None;
        self.changes = Value::Null;
        if !self.connected {
            self.changes_error = Some("Reconnect to read changes.".into());
            cx.notify();
            return;
        }
        let mut params = self.review_scope.params();
        params["cwd"] = json!(self.cwd());
        params["chatId"] = json!(self.selected.as_ref().map(|c| c["id"].clone()));
        params["targetDeviceId"] = json!(self.selected.as_ref().map(|c| c["deviceId"].clone()));
        let id = self.call(methods::GET_CHECKOUT_DIFF, params);
        self.changes_request = Some(id);
        cx.notify();
    }
    pub(super) fn open_review(&mut self, path: String, cx: &mut Context<Self>) {
        self.reset_review();
        self.review.path = Some(path.clone());
        if !self.changes["checkoutId"].is_string() {
            self.review.error = Some("Refresh the changes before opening a diff.".into());
            cx.notify();
            return;
        }
        if !self.connected {
            self.review.error = Some("Reconnect to read this diff.".into());
            cx.notify();
            return;
        }
        let mut params = self.review_scope.params();
        params["checkoutId"] = self.changes["checkoutId"].clone();
        params["cwd"] = self.changes["cwd"].clone();
        params["path"] = json!(path);
        params["diffChecksum"] = self.changes["checksum"].clone();
        params["chatId"] = json!(self.selected.as_ref().map(|c| c["id"].clone()));
        params["targetDeviceId"] = json!(self.selected.as_ref().map(|c| c["deviceId"].clone()));
        let id = self.call(methods::GET_CHECKOUT_FILE_DIFF_TEXT, params);
        self.review.request = Some(id);
        cx.notify();
    }
    pub(super) fn receive_review(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let summary = self.changes_request == Some(id);
        if !summary && self.review.request != Some(id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.pending.remove(&id);
        if summary {
            self.changes_request = None;
            if let Some(error) = frame["err"].as_str() {
                self.changes_error = Some(error.into());
            } else if let Some(value) = frame.get("ok") {
                self.changes = Value::Object(
                    value
                        .as_object()
                        .map(|object| {
                            object
                                .iter()
                                .filter(|(key, _)| key.as_str() != "patch")
                                .map(|(key, value)| (key.clone(), value.clone()))
                                .collect()
                        })
                        .unwrap_or_default(),
                );
                self.changes_error = None;
            }
        } else {
            self.review.request = None;
            if let Some(error) = frame["err"].as_str() {
                self.review.error = Some(error.into());
            } else {
                let value = &frame["ok"];
                if value["stale"] == true || value["diffChecksum"] != self.changes["checksum"] {
                    self.review.error = Some(
                        "Files changed during this review. Refresh to see the current diff.".into(),
                    );
                } else if value["binary"] == true {
                    self.review.notice =
                        Some("Binary file changed. A text diff is unavailable.".into());
                } else if value["truncated"] == true {
                    self.review.notice =
                        Some("This file exceeds the text diff preview limit.".into());
                } else {
                    let old = value["oldText"].as_str().unwrap_or_default();
                    let new = value["newText"].as_str().unwrap_or_default();
                    self.review.can_open = value["newText"].is_string();
                    if old.lines().count().saturating_add(new.lines().count()) > 100_000 {
                        self.review.notice =
                            Some("This diff has too many lines for an inline preview.".into());
                    } else {
                        self.review.lines = Arc::new(diff_lines(old, new));
                    }
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn workspace_changes_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut view = column()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                row()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .child(label(if self.review.path.is_some() {
                        "File changes"
                    } else if matches!(self.review_scope, ReviewScope::LatestTurn) {
                        "Latest turn"
                    } else if !self.review_scope.working() {
                        "Commit changes"
                    } else if self
                        .selected
                        .as_ref()
                        .is_some_and(|chat| chat["archived"] == true)
                    {
                        "Saved changes"
                    } else {
                        "Working tree"
                    }))
                    .child(div().flex_1())
                    .child(
                        control("refresh-workspace-changes", "Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_changes(cx))),
                    ),
            )
            .when_some(self.changes_error.clone(), |d, error| {
                d.child(label(error).p_3())
            });
        if !matches!(self.review_scope, ReviewScope::Commit { .. }) {
            view = view.child(
                row()
                    .px_3()
                    .gap_2()
                    .child(
                        control("review-working-tree", "Working tree")
                            .when(self.review_scope.working(), |d| d.bg(rgb(0x222222)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.review_scope = ReviewScope::WorkingTree;
                                this.refresh_changes(cx);
                            })),
                    )
                    .child(
                        control("review-latest-turn", "Latest turn")
                            .when(matches!(self.review_scope, ReviewScope::LatestTurn), |d| {
                                d.bg(rgb(0x222222))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.review_scope = ReviewScope::LatestTurn;
                                this.refresh_changes(cx);
                            })),
                    ),
            );
        }
        if let ReviewScope::Commit { sha, subject } = &self.review_scope {
            view = view.child(
                column()
                    .px_3()
                    .gap_2()
                    .child(
                        row()
                            .gap_2()
                            .child(control("back-to-history", "History").on_click(
                                cx.listener(|this, _, _, cx| this.set_panel("history", cx)),
                            ))
                            .child(control("working-tree-scope", "Working tree").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.review_scope = ReviewScope::WorkingTree;
                                    this.refresh_changes(cx);
                                }),
                            )),
                    )
                    .child(
                        label(format!(
                            "{} · {}",
                            sha.chars().take(8).collect::<String>(),
                            subject
                        ))
                        .text_ellipsis()
                        .text_size(px(11.)),
                    ),
            );
        }
        if let Some(path) = &self.review.path {
            let path = path.clone();
            view = view
                .child(
                    row()
                        .px_3()
                        .gap_2()
                        .child(control("back-to-changes", "Back").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.reset_review();
                                cx.notify();
                            },
                        )))
                        .child(label(path.clone()).text_ellipsis().flex_1().min_w_0()),
                )
                .when_some(self.review.error.clone(), |d, error| {
                    d.child(label(error).p_3())
                })
                .when_some(self.review.notice.clone(), |d, notice| {
                    d.child(label(notice).p_3())
                })
                .when(self.review.request.is_some(), |d| {
                    d.child(label("Loading diff…").p_3())
                });
            let lines = self.review.lines.clone();
            if !lines.is_empty() {
                view = view.child(
                    uniform_list("workspace-diff-lines", lines.len(), move |range, _, _| {
                        range
                            .map(|index| {
                                let line = &lines[index];
                                row()
                                    .h(px(20.))
                                    .px_2()
                                    .gap_2()
                                    .font_family("Geist Mono")
                                    .text_size(px(11.))
                                    .bg(rgb(match line.kind {
                                        -1 => 0x291719,
                                        1 => 0x14291e,
                                        _ => 0x111111,
                                    }))
                                    .child(
                                        div()
                                            .w(px(10.))
                                            .flex_shrink_0()
                                            .text_color(rgb(match line.kind {
                                                -1 => 0xe5a0a0,
                                                1 => 0x8ccca4,
                                                _ => 0x777777,
                                            }))
                                            .child(match line.kind {
                                                -1 => "−",
                                                1 => "+",
                                                _ => " ",
                                            }),
                                    )
                                    .child(div().whitespace_nowrap().child(line.text.clone()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .track_scroll(&self.review.scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained),
                );
            }
            view = view
                .when(self.review.lines.is_empty(), |d| d.child(div().flex_1()))
                .when(
                    self.review.can_open
                        && self.selected.is_some()
                        && !matches!(self.review_scope, ReviewScope::Commit { .. }),
                    |d| {
                        d.child(
                            control("edit-reviewed-file", "Open in editor")
                                .m_3()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.file_navigation_blocked(cx) {
                                        return;
                                    }
                                    this.right_tab = "files".into();
                                    this.read_file(&path, false, cx);
                                })),
                        )
                    },
                );
        } else {
            let files = self.changes["files"].as_array();
            view = view
                .child(
                    row()
                        .px_3()
                        .gap_2()
                        .child(label(format!("{} files", files.map_or(0, Vec::len))))
                        .child(
                            label(format!(
                                "+{}",
                                self.changes["additions"].as_u64().unwrap_or(0)
                            ))
                            .text_color(rgb(0x8ccca4)),
                        )
                        .child(
                            label(format!(
                                "−{}",
                                self.changes["deletions"].as_u64().unwrap_or(0)
                            ))
                            .text_color(rgb(0xe5a0a0)),
                        ),
                )
                .when(self.changes["truncated"] == true, |d| {
                    d.child(label("Partial snapshot — refresh to check for more changes.").p_3())
                })
                .when(self.changes_request.is_some(), |d| {
                    d.child(label("Checking changes…").p_3())
                })
                .child(
                    column()
                        .id("changed-files")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_2()
                        .gap_1()
                        .when(
                            files.is_some_and(Vec::is_empty) && self.changes_request.is_none(),
                            |d| {
                                d.child(
                                    label(if self.review_scope.working() {
                                        "No working tree changes"
                                    } else if matches!(self.review_scope, ReviewScope::LatestTurn) {
                                        "No changes since the latest turn"
                                    } else {
                                        "This commit has no file changes"
                                    })
                                    .p_3(),
                                )
                            },
                        )
                        .children(
                            files
                                .into_iter()
                                .flatten()
                                .enumerate()
                                .map(|(index, file)| {
                                    let path = file["path"].as_str().unwrap_or_default().to_owned();
                                    row()
                                        .id(("review-change", index))
                                        .h(px(32.))
                                        .px_2()
                                        .gap_2()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|d| d.bg(rgb(0x222222)))
                                        .child(icon("file-code").size(px(13.)))
                                        .child(
                                            label(path.clone()).flex_1().min_w_0().text_ellipsis(),
                                        )
                                        .child(
                                            label(format!(
                                                "+{}",
                                                file["additions"].as_u64().unwrap_or(0)
                                            ))
                                            .text_size(px(10.))
                                            .text_color(rgb(0x8ccca4)),
                                        )
                                        .child(
                                            label(format!(
                                                "−{}",
                                                file["deletions"].as_u64().unwrap_or(0)
                                            ))
                                            .text_size(px(10.))
                                            .text_color(rgb(0xe5a0a0)),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_review(path.clone(), cx)
                                        }))
                                }),
                        ),
                );
        }
        view.into_any_element()
    }
}
