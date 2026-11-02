//! Paginated host-owned Git history and pinned commit review.
use super::*;
use crate::localization::message as tr;
use crate::shell::{MUTED, RAISED, TEXT, column, control, label, row};
use std::sync::Arc;
use zuse_i18n::messages as msg;
use zuse_protocol::{
    GitHistoryCommit, GitHistoryPage,
    presentation::git_history::{GraphLayout, GraphRow, SegmentShape, layout_graph},
};

pub(super) struct History {
    input: Entity<input::TextInput>,
    _submit: Subscription,
    query: String,
    commits: Arc<Vec<GitHistoryCommit>>,
    graph: Arc<GraphLayout>,
    pub(super) page: Option<GitHistoryPage>,
    request: Option<(u64, bool, Value)>,
    error: Option<String>,
    scroll: UniformListScrollHandle,
}
impl History {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        let input =
            cx.new(|cx| input::TextInput::new("Search commits, authors or SHA", window, cx));
        let submit = cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
            this.search_history(cx)
        });
        Self {
            input,
            _submit: submit,
            query: String::new(),
            commits: Arc::new(vec![]),
            graph: Arc::new(GraphLayout::default()),
            page: None,
            request: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
        }
    }
}
impl WorkspaceView {
    pub(super) fn rebind_history_review(&mut self, cx: &mut Context<Self>) {
        self.reset_history(cx);
        if let Some(id) = self.changes_request.take() {
            self.cancel(id);
        }
        self.reset_review();
        self.changes = Value::Null;
        self.changes_error = None;
        if self.right_open {
            if self.right_tab == "history" {
                self.refresh_history(cx);
            }
            if self.right_tab == "changes" {
                self.refresh_changes(cx);
            }
        }
    }
    fn history_context(&self) -> Value {
        json!({"cwd":self.cwd(),"chatId":self.selected.as_ref().map(|c|c["id"].clone()),"targetDeviceId":self.selected.as_ref().map(|c|c["deviceId"].clone()),"archived":self.selected.as_ref().map(|c|c["archived"].clone())})
    }
    pub(super) fn reset_history(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _, _)) = self.history.request.take() {
            self.cancel(id);
        }
        self.history.commits = Arc::new(vec![]);
        self.history.graph = Arc::new(GraphLayout::default());
        self.history.page = None;
        self.history.query.clear();
        self.history.error = None;
        self.history.scroll = UniformListScrollHandle::new();
        self.history
            .input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
    pub(super) fn interrupt_history(&mut self) {
        if self.history.request.take().is_some() {
            self.history.error = Some("History read interrupted. Refresh when connected.".into());
        }
    }
    fn clear_history_search(&mut self, cx: &mut Context<Self>) {
        self.history
            .input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.history.query.clear();
        self.refresh_history(cx);
    }
    fn search_history(&mut self, cx: &mut Context<Self>) {
        self.history.query = self.history.input.read(cx).value(cx).trim().to_owned();
        self.refresh_history(cx);
    }
    pub(super) fn refresh_history(&mut self, cx: &mut Context<Self>) {
        self.load_history(false, cx);
    }
    fn load_history(&mut self, append: bool, cx: &mut Context<Self>) {
        if !self.connected {
            self.history.error = Some("Reconnect to load history.".into());
            cx.notify();
            return;
        }
        if self.cwd().is_empty() {
            self.history.error = Some("Open a project or thread to see its history.".into());
            cx.notify();
            return;
        }
        if append && self.history.request.is_some() {
            return;
        }
        let cursor = if append {
            let Some(cursor) = self.history.page.as_ref().and_then(|p| p.next_cursor) else {
                return;
            };
            cursor
        } else {
            0
        };
        if let Some((id, _, _)) = self.history.request.take() {
            self.cancel(id);
        }
        if !append {
            self.history.commits = Arc::new(vec![]);
            self.history.page = None;
            self.history.graph = Arc::new(GraphLayout::default());
            self.history.scroll = UniformListScrollHandle::new();
        }
        let context = self.history_context();
        let mut params = context.clone();
        params.as_object_mut().unwrap().remove("archived");
        params["cursor"] = json!(cursor);
        params["limit"] = json!(100);
        let method = if self.history.query.is_empty() {
            methods::LIST_GIT_HISTORY
        } else {
            params["query"] = json!(self.history.query);
            methods::SEARCH_GIT_HISTORY
        };
        self.history.error = None;
        let id = self.call(method, params);
        self.history.request = Some((id, append, context));
        cx.notify();
    }
    pub(super) fn receive_history(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((pending, append, context)) = self.history.request.clone() else {
            return false;
        };
        if pending != id {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.history.request = None;
        self.pending.remove(&id);
        if context != self.history_context() {
            self.reset_history(cx);
            if self.right_open && self.right_tab == "history" {
                self.refresh_history(cx);
            }
            return true;
        }
        if let Some(error) = frame["err"].as_str() {
            self.history.error = Some(error.into());
        } else {
            match serde_json::from_value::<GitHistoryPage>(frame["ok"].clone()) {
                Ok(mut page) => {
                    if append
                        && self
                            .history
                            .page
                            .as_ref()
                            .is_some_and(|previous| previous.head_sha != page.head_sha)
                    {
                        self.refresh_history(cx);
                        self.history.error = Some(
                            "History changed while loading. Showing the refreshed history.".into(),
                        );
                        return true;
                    }
                    if append && let Some(previous) = &self.history.page {
                        page.total_count = page.total_count.or(previous.total_count);
                        page.head_commit_count =
                            page.head_commit_count.or(previous.head_commit_count);
                        if page.comparison.is_none() {
                            page.comparison = previous.comparison.clone();
                        }
                        if page.branch_tips.is_empty() {
                            page.branch_tips = previous.branch_tips.clone();
                        }
                    }
                    let rows = Arc::make_mut(&mut self.history.commits);
                    let mut seen: std::collections::HashSet<String> =
                        rows.iter().map(|c| c.sha.clone()).collect();
                    rows.extend(
                        page.commits
                            .iter()
                            .filter(|c| seen.insert(c.sha.clone()))
                            .cloned(),
                    );
                    self.history.graph = Arc::new(layout_graph(rows, page.head_sha.as_deref()));
                    self.history.page = Some(page);
                    self.history.error = None;
                }
                Err(_) => {
                    self.history.error =
                        Some("Could not read Git history. Refresh to try again.".into())
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn open_history_commit(&mut self, commit: GitHistoryCommit, cx: &mut Context<Self>) {
        self.review_scope = workspace_review::ReviewScope::Commit {
            sha: commit.sha,
            subject: commit.subject,
        };
        self.right_tab = "changes".into();
        self.right_open = true;
        self.refresh_changes(cx);
    }
    pub(super) fn history_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.history.request.is_some();
        let count = self
            .history
            .page
            .as_ref()
            .and_then(|p| p.total_count)
            .map(|n| n.to_string())
            .unwrap_or_else(|| self.history.commits.len().to_string());
        let search = row()
            .px_3()
            .gap_1()
            .child(div().flex_1().min_w_0().child(self.history.input.clone()))
            .child(
                control("search-history", tr(cx, msg::common::search))
                    .on_click(cx.listener(|this, _, _, cx| this.search_history(cx))),
            )
            .when(!self.history.query.is_empty(), |d| {
                d.child(
                    control(
                        "clear-history-search",
                        tr(cx, msg::settings::acp_agents_clear_variables),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_history_search(cx);
                    })),
                )
            });
        let header = row()
            .h(px(40.))
            .px_3()
            .gap_2()
            .child(label(
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|c| c["archived"] == true)
                {
                    ("Saved history").to_string()
                } else {
                    crate::localization::text("native-history", cx)
                },
            ))
            .child(div().flex_1())
            .child(
                control(
                    "refresh-history",
                    if busy {
                        crate::localization::text("common-loading", cx)
                    } else {
                        crate::localization::text("chat-attempts_refresh", cx)
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh_history(cx))),
            );
        let metadata = row()
            .px_3()
            .gap_2()
            .child(
                label(format!("{count} commits"))
                    .text_size(px(11.))
                    .text_color(rgb(MUTED)),
            )
            .when_some(
                self.history
                    .page
                    .as_ref()
                    .and_then(|p| p.comparison.clone()),
                |d, c| {
                    d.child(
                        label(format!("↑{} ↓{} · {}", c.ahead, c.behind, c.base))
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED)),
                    )
                },
            );
        let mut view = column()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(header)
            .child(search)
            .when_some(self.history.error.clone(), |d, error| {
                d.child(label(error).px_3().text_size(px(11.)))
            })
            .child(metadata);
        if self.history.commits.is_empty() {
            view = view.child(
                label(if busy {
                    crate::localization::text("native-loading_history", cx)
                } else if self.history.query.is_empty() {
                    ("No commits to show").to_string()
                } else {
                    ("No matching commits").to_string()
                })
                .p_3(),
            );
        }
        let graph_width = (self.history.graph.max_lane_count.max(1) as f32 * 12. + 12.).min(96.);
        view.child(
            uniform_list(
                "git-history-rows",
                self.history.commits.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| {
                            let commit = this.history.commits.get(index)?.clone();
                            let graph = this.history.graph.rows.get(index)?.clone();
                            let refs = commit
                                .refs
                                .iter()
                                .map(|r| r.label.clone())
                                .collect::<Vec<_>>()
                                .join(" · ");
                            let meta = format!(
                                "{} · {} · {}",
                                commit.sha.chars().take(8).collect::<String>(),
                                commit.author_name,
                                commit.authored_at.chars().take(10).collect::<String>()
                            );
                            Some(
                                row()
                                    .id(("history-commit", index))
                                    .h(px(64.))
                                    .w_full()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgb(RAISED)))
                                    .child(graph_cell(
                                        graph,
                                        graph_width,
                                        this.history.graph.max_lane_count,
                                    ))
                                    .child(
                                        column()
                                            .flex_1()
                                            .min_w_0()
                                            .justify_center()
                                            .gap(px(3.))
                                            .pr_3()
                                            .child(
                                                label(commit.subject.clone())
                                                    .text_color(rgb(TEXT))
                                                    .text_ellipsis(),
                                            )
                                            .when(!refs.is_empty(), |d| {
                                                d.child(
                                                    label(refs)
                                                        .text_color(rgb(0x96b9a3))
                                                        .text_size(px(10.))
                                                        .text_ellipsis(),
                                                )
                                            })
                                            .child(
                                                label(meta)
                                                    .text_color(rgb(MUTED))
                                                    .text_size(px(10.))
                                                    .text_ellipsis(),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_history_commit(commit.clone(), cx)
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.history.scroll)
            .flex_1()
            .min_h_0(),
        )
        .when(
            self.history
                .page
                .as_ref()
                .is_some_and(|p| p.next_cursor.is_some()),
            |d| {
                d.child(
                    control(
                        "load-more-history",
                        if busy {
                            crate::localization::text("common-loading", cx)
                        } else {
                            ("Load more commits").to_string()
                        },
                    )
                    .m_3()
                    .on_click(cx.listener(|this, _, _, cx| this.load_history(true, cx))),
                )
            },
        )
        .into_any_element()
    }
}
const COLORS: [u32; 6] = [0x8db3a0, 0xbba2cf, 0xc8ae83, 0x84adc7, 0xc68c98, 0x9daf7d];
fn graph_cell(row: GraphRow, width: f32, lanes: usize) -> Div {
    let spacing = if lanes <= 1 {
        12.
    } else {
        12f32.min((width - 18.) / (lanes - 1) as f32)
    };
    let x = move |lane: usize| 9. + lane as f32 * spacing;
    let node_x = x(row.node_lane);
    let color = COLORS[row.node_color_id % COLORS.len()];
    let head = row.is_head;
    div()
        .relative()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    for segment in &row.segments {
                        let mut path = PathBuilder::stroke(px(1.5));
                        let (from_y, to_y) = match segment.shape {
                            SegmentShape::Incoming => (0., 32.),
                            SegmentShape::Outgoing => (32., 64.),
                            SegmentShape::Through => (0., 64.),
                        };
                        let from = point(
                            bounds.left() + px(x(segment.from_lane)),
                            bounds.top() + px(from_y),
                        );
                        let to = point(
                            bounds.left() + px(x(segment.to_lane)),
                            bounds.top() + px(to_y),
                        );
                        path.move_to(from);
                        if segment.from_lane == segment.to_lane {
                            path.line_to(to);
                        } else {
                            let mid = bounds.top() + px((from_y + to_y) / 2.);
                            path.cubic_bezier_to(to, point(from.x, mid), point(to.x, mid));
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, rgb(COLORS[segment.color_id % COLORS.len()]));
                        }
                    }
                },
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top(px(28.))
                .left(px(node_x - 4.))
                .size(px(8.))
                .rounded_full()
                .bg(rgb(color))
                .when(head, |d| d.border_2().border_color(rgb(TEXT))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    fn commit(sha: &str) -> GitHistoryCommit {
        GitHistoryCommit {
            sha: sha.into(),
            parent_shas: vec![],
            subject: format!("Commit {sha}"),
            author_name: "Author".into(),
            author_email: "author@localhost".into(),
            authored_at: "2026-10-18T00:00:00Z".into(),
            refs: vec![],
        }
    }
    fn page(rows: Vec<GitHistoryCommit>, head: &str, cursor: Option<usize>) -> Value {
        serde_json::to_value(GitHistoryPage {
            commits: rows,
            branch_tips: vec![],
            head_sha: Some(head.into()),
            next_cursor: cursor,
            total_count: Some(3),
            head_commit_count: Some(3),
            comparison: None,
        })
        .unwrap()
    }
    fn fixture(cx: &mut TestAppContext) -> (WindowHandle<WorkspaceView>, UnboundedReceiver<Value>) {
        cx.update(initialize);
        let (requests, rx) = futures::channel::mpsc::unbounded();
        let (_, events) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(Transport { requests, events }, window, cx)
        });
        window
            .update(cx, |view, _, _| {
                view.connected = true;
                view.selected =
                    Some(json!({"id":"chat","cwd":"/repo","deviceId":"host","archived":true}));
                view.right_tab = "history".into();
                view.right_open = true;
            })
            .unwrap();
        (window, rx)
    }
    #[gpui::test]
    fn history_pages_search_and_commit_source_stay_on_the_execution_host(cx: &mut TestAppContext) {
        let (window, mut requests) = fixture(cx);
        window.update(cx,|view,_,cx| {
            view.refresh_history(cx); let first=view.history.request.as_ref().unwrap().0;
            view.receive(json!({"id":first,"ok":page(vec![commit("a")],"a",Some(1))}),cx);
            view.load_history(true,cx);let second=view.history.request.as_ref().unwrap().0;
            let mut next = page(vec![commit("a"),commit("b")],"a",None);
            next["totalCount"] = Value::Null; next["headCommitCount"] = Value::Null;
            view.receive(json!({"id":second,"ok":next}),cx);
            assert_eq!(view.history.page.as_ref().unwrap().total_count,Some(3));
            assert_eq!(view.history.commits.len(),2,"overlapping pages must not duplicate rows");
            view.open_history_commit(commit("b"),cx);let diff=view.changes_request.unwrap();
            view.receive(json!({"id":diff,"ok":{"checkoutId":"saved","cwd":"/repo","checksum":"pinned","files":[{"path":"file.rs"}],"additions":1,"deletions":0}}),cx);
            view.open_review("file.rs".into(),cx);
            view.history.input.update(cx,|input,cx|input.set_value("Author".into(),cx));
        }).unwrap();
        window
            .update(cx, |view, _, cx| {
                view.search_history(cx);
                assert_eq!(view.history.query, "Author");
                view.clear_history_search(cx);
                assert!(view.history.query.is_empty());
                view.review_scope=workspace_review::ReviewScope::LatestTurn;
                view.refresh_changes(cx);let diff=view.changes_request.unwrap();
                view.receive(json!({"id":diff,"ok":{"checkoutId":"saved","cwd":"/repo","checksum":"turn","files":[{"path":"turn.rs"}]}}),cx);
                view.open_review("turn.rs".into(),cx);
            })
            .unwrap();
        let mut calls = vec![];
        while let Ok(frame) = requests.try_recv() {
            if frame["method"].is_string() {
                calls.push(frame);
            }
        }
        for call in calls.iter().filter(|c| {
            matches!(
                c["method"].as_str(),
                Some(
                    methods::LIST_GIT_HISTORY
                        | methods::SEARCH_GIT_HISTORY
                        | methods::GET_CHECKOUT_DIFF
                        | methods::GET_CHECKOUT_FILE_DIFF_TEXT
                )
            )
        }) {
            assert_eq!(call["params"]["targetDeviceId"], "host");
            assert_eq!(call["params"]["chatId"], "chat");
        }
        assert!(
            calls
                .iter()
                .any(|c| c["method"] == methods::LIST_GIT_HISTORY && c["params"]["cursor"] == 1)
        );
        assert!(calls.iter().any(
            |c| c["method"] == methods::SEARCH_GIT_HISTORY && c["params"]["query"] == "Author"
        ));
        for method in [
            methods::GET_CHECKOUT_DIFF,
            methods::GET_CHECKOUT_FILE_DIFF_TEXT,
        ] {
            let request = calls.iter().find(|c| c["method"] == method).unwrap();
            assert_eq!(request["params"]["mode"], "commit");
            assert_eq!(request["params"]["commitSha"], "b");
            assert!(calls.iter().any(|c| c["method"] == method
                && c["params"]["mode"] == "turn"
                && c["params"]["commitSha"].is_null()));
        }
    }
    #[gpui::test]
    fn changed_heads_and_archived_contexts_reject_stale_history_pages(cx: &mut TestAppContext) {
        let (window, _requests) = fixture(cx);
        window
            .update(cx, |view, _, cx| {
                view.refresh_history(cx);
                let first = view.history.request.as_ref().unwrap().0;
                view.receive(
                    json!({"id":first,"ok":page(vec![commit("a")],"a",Some(1))}),
                    cx,
                );
                view.load_history(true, cx);
                let second = view.history.request.as_ref().unwrap().0;
                view.receive(
                    json!({"id":second,"ok":page(vec![commit("old")],"new-head",None)}),
                    cx,
                );
                assert!(view.history.commits.is_empty());
                let refresh = view.history.request.as_ref().unwrap().0;
                view.selected.as_mut().unwrap()["archived"] = json!(false);
                view.receive(
                    json!({"id":refresh,"ok":page(vec![commit("late-archive")],"a",None)}),
                    cx,
                );
                assert!(view.history.commits.is_empty());
                let rebound = view.history.request.as_ref().unwrap().0;
                assert_ne!(rebound, refresh);
                view.receive(
                    json!({"id":rebound,"ok":page(vec![commit("current")],"current",None)}),
                    cx,
                );
                assert_eq!(view.history.commits[0].sha, "current");
                view.reset_history(cx);
                view.receive(
                    json!({"id":rebound,"ok":page(vec![commit("late-reset")],"old",None)}),
                    cx,
                );
                assert!(view.history.commits.is_empty());
            })
            .unwrap();
    }
}
