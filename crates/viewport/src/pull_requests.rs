//! Browser PR board/inspection through the owning engine's source-control service.
use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, label, markdown_block, row};
use std::{collections::HashSet, sync::Arc};
use zuse_i18n::messages as msg;
use zuse_protocol::{
    ChangeRequestDetail, ChangeRequestFilter, ChangeRequestListItem, ChangeRequestPage,
};
#[derive(Clone, Copy)]
enum Read {
    Repository,
    Page(bool),
    Detail,
    Diff,
}
pub(super) struct PullRequests {
    repository: Option<String>,
    filter: ChangeRequestFilter,
    items: Arc<Vec<ChangeRequestListItem>>,
    next: Option<String>,
    total: Option<u64>,
    reads: HashMap<u64, Read>,
    watch: Option<u64>,
    watch_seeded: bool,
    /// The error the live watch last showed, so recovery clears only that.
    watch_error: Option<String>,
    selected: Option<String>,
    detail: Option<ChangeRequestDetail>,
    patch: Arc<Vec<String>>,
    tab: &'static str,
    comment: Entity<input::TextInput>,
    posting: Option<(u64, String, String)>,
    drafts: HashMap<String, String>,
    uncertain: HashSet<String>,
    error: Option<String>,
    notice: Option<String>,
    scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
}
impl PullRequests {
    pub(super) fn new(window: &mut Window, cx: &mut Context<WorkspaceView>) -> Self {
        Self {
            repository: None,
            filter: ChangeRequestFilter::Authored,
            items: Arc::new(vec![]),
            next: None,
            total: None,
            reads: HashMap::new(),
            watch: None,
            watch_seeded: false,
            watch_error: None,
            selected: None,
            detail: None,
            patch: Arc::new(vec![]),
            tab: "overview",
            comment: cx.new(|cx| input::TextInput::multiline("Write a comment…", window, cx)),
            posting: None,
            drafts: HashMap::new(),
            uncertain: HashSet::new(),
            error: None,
            notice: None,
            scroll: Default::default(),
            diff_scroll: Default::default(),
        }
    }
}
impl WorkspaceView {
    fn pr_params(&self, mut value: Value) -> Value {
        value["targetDeviceId"] = json!(
            self.selected
                .as_ref()
                .map(|c| c["deviceId"].clone())
                .or_else(|| self.project.as_ref().map(|p| p["deviceId"].clone()))
        );
        value
    }
    fn pr_read(&mut self, kind: Read, method: &str, params: Value) {
        if !self.connected {
            self.pull_requests.error = Some("Reconnect to read pull requests.".into());
            return;
        }
        let id = self.call(method, self.pr_params(params));
        self.pull_requests.reads.insert(id, kind);
    }
    fn save_pr_draft(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.pull_requests.selected.clone() {
            let text = self.pull_requests.comment.read(cx).value(cx);
            if text.is_empty() {
                self.pull_requests.drafts.remove(&url);
            } else {
                if self.pull_requests.drafts.len() >= 32
                    && !self.pull_requests.drafts.contains_key(&url)
                {
                    if let Some(key) = self.pull_requests.drafts.keys().next().cloned() {
                        self.pull_requests.drafts.remove(&key);
                    }
                }
                self.pull_requests.drafts.insert(url, text);
            }
        }
    }
    pub(super) fn reset_pull_requests(&mut self, cx: &mut Context<Self>) {
        self.save_pr_draft(cx);
        self.interrupt_pull_requests();
        self.pull_requests.repository = None;
        self.pull_requests.items = Arc::new(vec![]);
        self.pull_requests.next = None;
        self.pull_requests.total = None;
        self.pull_requests.selected = None;
        self.pull_requests.detail = None;
        self.pull_requests.patch = Arc::new(vec![]);
        self.pull_requests.error = None;
        self.pull_requests.notice = None;
    }
    pub(super) fn interrupt_pull_requests(&mut self) {
        if let Some(id) = self.pull_requests.watch.take() {
            self.cancel(id);
        }
        for id in std::mem::take(&mut self.pull_requests.reads).into_keys() {
            self.cancel(id);
        }
        if let Some((id, url, body)) = self.pull_requests.posting.take() {
            self.cancel(id);
            self.pull_requests.drafts.entry(url.clone()).or_insert(body);
            self.pull_requests.uncertain.insert(url);
            self.pull_requests.notice = Some(
                "Comment delivery is uncertain. Refresh the discussion before posting again."
                    .into(),
            );
        }
    }
    pub(super) fn refresh_pull_requests(&mut self, cx: &mut Context<Self>) {
        if !self.pull_requests.reads.is_empty() || self.pull_requests.posting.is_some() {
            return;
        }
        self.pull_requests.error = None;
        if self.pull_requests.selected.is_some() {
            self.refresh_pr_detail();
        } else if self.pull_requests.repository.is_some() {
            self.load_pr_page(false);
        } else {
            let cwd = if self
                .selected
                .as_ref()
                .is_some_and(|c| c["archived"] == true)
            {
                self.selected
                    .as_ref()
                    .and_then(|c| c["spaceId"].as_str())
                    .and_then(|id| self.spaces.iter().find(|s| s.id == id))
                    .map(|s| s.path.clone())
                    .unwrap_or_else(|| self.cwd())
            } else {
                self.cwd()
            };
            if cwd.is_empty() {
                self.pull_requests.error =
                    Some("Select a project to browse its pull requests.".into());
            } else {
                self.pr_read(
                    Read::Repository,
                    methods::GET_CHANGE_REQUEST_REPOSITORY,
                    json!({"cwd":cwd}),
                );
            }
        }
        cx.notify();
    }
    fn load_pr_page(&mut self, append: bool) {
        let Some(repository) = self.pull_requests.repository.clone() else {
            return;
        };
        self.pr_read(Read::Page(append),methods::LIST_CHANGE_REQUEST_PAGE,json!({"repository":repository,"filter":self.pull_requests.filter,"after":if append{self.pull_requests.next.clone()}else{None},"refresh":true}));
    }
    fn open_pr(&mut self, item: ChangeRequestListItem, cx: &mut Context<Self>) {
        if self.pull_requests.posting.is_some() {
            return;
        }
        self.save_pr_draft(cx);
        for id in std::mem::take(&mut self.pull_requests.reads).into_keys() {
            self.cancel(id);
        }
        if let Some(id) = self.pull_requests.watch.take() {
            self.cancel(id);
        }
        let watch = self.call(
            methods::WATCH_CHANGE_REQUEST,
            self.pr_params(json!({"url":item.url})),
        );
        self.pull_requests.watch = Some(watch);
        self.pull_requests.watch_seeded = false;
        self.pull_requests.selected = Some(item.url.clone());
        self.pull_requests.detail = None;
        self.pull_requests.patch = Arc::new(vec![]);
        self.pull_requests.tab = "overview";
        self.pull_requests.error = None;
        self.pull_requests.notice = self.pull_requests.uncertain.contains(&item.url).then(|| {
            "Comment delivery was uncertain. Check the discussion before posting again.".into()
        });
        let draft = self
            .pull_requests
            .drafts
            .get(&item.url)
            .cloned()
            .unwrap_or_default();
        self.pull_requests
            .comment
            .update(cx, |input, cx| input.set_value(draft, cx));
        self.pr_read(
            Read::Detail,
            methods::GET_CHANGE_REQUEST,
            json!({"url":item.url,"headRefOid":item.head_ref_oid,"refresh":true}),
        );
        cx.notify();
    }
    fn refresh_pr_detail(&mut self) {
        let Some(url) = self.pull_requests.selected.clone() else {
            return;
        };
        if self.pull_requests.watch.is_none() && self.connected {
            let id = self.call(
                methods::WATCH_CHANGE_REQUEST,
                self.pr_params(json!({"url":url})),
            );
            self.pull_requests.watch = Some(id);
        }
        self.pr_read(
            Read::Detail,
            methods::GET_CHANGE_REQUEST,
            json!({"url":url,"refresh":true}),
        );
    }
    fn pr_tab(&mut self, tab: &'static str, cx: &mut Context<Self>) {
        self.pull_requests.tab = tab;
        if tab == "diff"
            && self.pull_requests.patch.is_empty()
            && !self
                .pull_requests
                .reads
                .values()
                .any(|r| matches!(r, Read::Diff))
        {
            if let Some(detail) = &self.pull_requests.detail {
                let params = json!({"url":detail.url,"headRefOid":detail.head_ref_oid,"baseRefOid":detail.base_ref_oid});
                self.pr_read(Read::Diff, methods::GET_CHANGE_REQUEST_DIFF, params);
            }
        }
        cx.notify();
    }
    fn post_pr_comment(&mut self, cx: &mut Context<Self>) {
        if !self.connected || self.pull_requests.posting.is_some() {
            return;
        }
        let Some(url) = self.pull_requests.selected.clone() else {
            return;
        };
        let body = self.pull_requests.comment.read(cx).value(cx);
        if body.trim().is_empty() || body.len() > 60000 {
            self.pull_requests.error = Some("Enter a comment of at most 60,000 bytes.".into());
            cx.notify();
            return;
        }
        let id = self.call(
            methods::POST_CHANGE_REQUEST_COMMENT,
            self.pr_params(json!({"url":url,"body":body})),
        );
        self.pull_requests.posting = Some((id, url, body));
        self.pull_requests.error = None;
        cx.notify();
    }
    pub(super) fn receive_pull_requests(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pull_requests.watch == Some(id) {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                if let Ok(status) = serde_json::from_value::<
                    zuse_protocol::git_workspace::SourceControlFrame,
                >(value.clone())
                {
                    if status.reset {
                        self.pull_requests.detail = None;
                        self.pull_requests.patch = Arc::new(vec![]);
                    }
                    reconcile_watch_error(
                        &mut self.pull_requests.error,
                        &mut self.pull_requests.watch_error,
                        status.error,
                    );
                    if status.refreshing {
                        cx.notify();
                        return true;
                    }
                    if !self.pull_requests.watch_seeded && status.snapshot.is_some() {
                        self.pull_requests.watch_seeded = true;
                        if self.pull_requests.detail.is_some() {
                            cx.notify();
                            return true;
                        }
                    }
                    let selected = self.pull_requests.selected.clone();
                    let mut revisions_changed = false;
                    if let Some(mut detail) = status
                        .snapshot
                        .and_then(|v| serde_json::from_value::<ChangeRequestDetail>(v).ok())
                        .filter(|detail| Some(&detail.url) == selected.as_ref())
                    {
                        if let Some(old) = &self.pull_requests.detail {
                            if old.head_ref_oid == detail.head_ref_oid
                                && old.base_ref_oid == detail.base_ref_oid
                            {
                                detail.review_threads = old.review_threads.clone();
                            } else {
                                revisions_changed = true;
                                self.pull_requests.patch = Arc::new(vec![]);
                                let stale: Vec<_> = self
                                    .pull_requests
                                    .reads
                                    .iter()
                                    .filter(|(_, read)| matches!(read, Read::Diff | Read::Detail))
                                    .map(|(id, _)| *id)
                                    .collect();
                                for stale in stale {
                                    self.pull_requests.reads.remove(&stale);
                                    self.cancel(stale);
                                }
                            }
                        }
                        self.pull_requests.detail = Some(detail);
                        if revisions_changed && self.pull_requests.tab == "diff" {
                            self.pr_tab("diff", cx);
                        }
                    }
                }
            }
            if frame.get("err").is_some() || frame["done"] == true {
                self.pull_requests.watch = None;
                self.pending.remove(&id);
                self.pull_requests.error =
                    Some("Live PR updates disconnected. Refresh to reconnect.".into());
            }
            cx.notify();
            return true;
        }
        let write = self
            .pull_requests
            .posting
            .as_ref()
            .is_some_and(|p| p.0 == id);
        let Some(kind) = self
            .pull_requests
            .reads
            .get(&id)
            .copied()
            .or(write.then_some(Read::Detail))
        else {
            return false;
        };
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.pending.remove(&id);
        self.pull_requests.reads.remove(&id);
        if write {
            let (_, url, body) = self.pull_requests.posting.take().unwrap();
            if frame
                .get("ok")
                .and_then(|v| {
                    serde_json::from_value::<zuse_protocol::ChangeRequestComment>(v.clone()).ok()
                })
                .is_some_and(|comment| !comment.id.is_empty())
            {
                if self.pull_requests.comment.read(cx).value(cx) == body {
                    self.pull_requests
                        .comment
                        .update(cx, |input, cx| input.set_value(String::new(), cx));
                    self.pull_requests.drafts.remove(&url);
                }
                self.pull_requests.uncertain.remove(&url);
                self.pull_requests.notice = Some("Comment posted.".into());
            } else {
                self.pull_requests.uncertain.insert(url);
                self.pull_requests.notice=Some("Comment delivery is uncertain. Check the refreshed discussion before posting again.".into());
            }
            self.refresh_pr_detail();
            cx.notify();
            return true;
        }
        let Some(value) = frame.get("ok") else {
            self.pull_requests.error=Some(frame["err"].as_str().unwrap_or("Could not read pull requests. Check GitHub CLI sign-in on the selected computer.").to_owned());
            cx.notify();
            return true;
        };
        match kind {
            Read::Repository => {
                if let Some(repository) = value.as_str().filter(|s| !s.is_empty()) {
                    self.pull_requests.repository = Some(repository.into());
                    self.load_pr_page(false);
                } else {
                    self.pull_requests.error =
                        Some("This project has no supported GitHub remote.".into());
                }
            }
            Read::Page(append) => {
                match serde_json::from_value::<ChangeRequestPage>(value.clone()) {
                    Ok(page) => {
                        let requested = self.pull_requests.repository.clone().unwrap_or_default();
                        let repository = page
                            .items
                            .first()
                            .map(|i| i.repository.clone())
                            .unwrap_or_else(|| requested.clone());
                        let valid = page.items.iter().all(|item| {
                            item.repository.eq_ignore_ascii_case(&repository)
                                && item.url
                                    == format!(
                                        "https://github.com/{}/pull/{}",
                                        item.repository, item.number
                                    )
                                && item.number > 0
                        });
                        if !valid || (append && !repository.eq_ignore_ascii_case(&requested)) {
                            self.pull_requests.error = Some(
                                "The PR page belongs to another repository. Refresh the project."
                                    .into(),
                            );
                        } else {
                            self.pull_requests.repository = Some(repository);
                            let mut items = if append {
                                (*self.pull_requests.items).clone()
                            } else {
                                vec![]
                            };
                            let mut known: HashSet<String> =
                                items.iter().map(|i| i.url.clone()).collect();
                            items.extend(
                                page.items
                                    .into_iter()
                                    .filter(|i| known.insert(i.url.clone())),
                            );
                            self.pull_requests.items = Arc::new(items);
                            self.pull_requests.next = page.next_cursor;
                            self.pull_requests.total = page.total_count;
                            self.pull_requests.error = None;
                        }
                    }
                    Err(_) => self.pull_requests.error = Some("Invalid pull request page.".into()),
                }
            }
            Read::Detail => match serde_json::from_value::<ChangeRequestDetail>(value.clone()) {
                Ok(detail)
                    if self.pull_requests.selected.as_deref() == Some(detail.url.as_str()) =>
                {
                    if self.pull_requests.detail.as_ref().is_none_or(|old| {
                        old.head_ref_oid != detail.head_ref_oid
                            || old.base_ref_oid != detail.base_ref_oid
                    }) {
                        self.pull_requests.patch = Arc::new(vec![]);
                        let stale: Vec<u64> = self
                            .pull_requests
                            .reads
                            .iter()
                            .filter(|(_, kind)| matches!(kind, Read::Diff))
                            .map(|(id, _)| *id)
                            .collect();
                        for id in stale {
                            self.pull_requests.reads.remove(&id);
                            self.cancel(id);
                        }
                    }
                    self.pull_requests.detail = Some(detail);
                    self.pull_requests.error = None;
                    if self.pull_requests.tab == "diff" {
                        self.pr_tab("diff", cx);
                    }
                }
                _ => {
                    self.pull_requests.error =
                        Some("Invalid or mismatched pull request detail.".into())
                }
            },
            Read::Diff => {
                if let Some(patch) = value.as_str() {
                    self.pull_requests.patch = Arc::new(patch.lines().map(str::to_owned).collect());
                } else {
                    self.pull_requests.error = Some("Invalid pull request diff.".into());
                }
            }
        }
        cx.notify();
        true
    }
    fn pr_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|i| {
                self.pull_requests
                    .items
                    .get(i)
                    .cloned()
                    .map(|item| (i, item))
            })
            .map(|(i, item)| {
                let selected = item.clone();
                column()
                    .id(("pr-row", i))
                    .h(px(82.))
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(crate::palette::rgb(0x343434))
                    .cursor_pointer()
                    .child(
                        row()
                            .gap_2()
                            .child(label(format!("#{}", item.number)))
                            .child(label(if item.is_draft {
                                crate::localization::text("projects-pr_pane_draft", cx).into()
                            } else {
                                (format!("{:?}", item.state)).to_string()
                            }))
                            .child(div().flex_1())
                            .child(label(format!("+{} −{}", item.additions, item.deletions))),
                    )
                    .child(div().text_size(px(12.)).truncate().child(item.title))
                    .child(label(item.author.login))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_pr(selected.clone(), cx)))
                    .into_any_element()
            })
            .collect()
    }
    fn pr_diff_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|i| self.pull_requests.patch.get(i))
            .map(|line| {
                let color = if line.starts_with('+') && !line.starts_with("+++") {
                    0x8ccca4
                } else if line.starts_with('-') && !line.starts_with("---") {
                    0xe5a0a0
                } else {
                    0xe8e9e7
                };
                div()
                    .h(px(19.))
                    .px_2()
                    .font_family("Geist Mono")
                    .text_size(px(11.))
                    .text_color(crate::palette::rgb(color))
                    .whitespace_nowrap()
                    .child(line.clone())
                    .into_any_element()
            })
            .collect()
    }
    pub(super) fn pull_requests_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = !self.pull_requests.reads.is_empty();
        let mut view = column()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                row()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .child(label(tr(cx, msg::native::pull_requests)))
                    .child(div().flex_1())
                    .child(
                        control(
                            "refresh-prs",
                            if busy {
                                crate::localization::text("common-loading", cx)
                            } else {
                                crate::localization::text("common-refresh", cx)
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_pull_requests(cx))),
                    ),
            )
            .when_some(self.pull_requests.error.clone(), |d, error| {
                d.child(label(error).p_3())
            })
            .when_some(self.pull_requests.notice.clone(), |d, notice| {
                d.child(label(notice).p_3())
            });
        if let Some(url) = self.pull_requests.selected.clone() {
            view = view.child(
                row()
                    .px_3()
                    .gap_2()
                    .child(
                        control("pr-back", tr(cx, msg::native::all_pull_requests)).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this.pull_requests.posting.is_some() {
                                    return;
                                }
                                this.save_pr_draft(cx);
                                if let Some(id) = this.pull_requests.watch.take() {
                                    this.cancel(id);
                                }
                                for id in std::mem::take(&mut this.pull_requests.reads).into_keys()
                                {
                                    this.cancel(id);
                                }
                                this.pull_requests.selected = None;
                                this.pull_requests.detail = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        control("pr-open-browser", tr(cx, msg::native::open_on_github))
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            );
            if let Some(detail) = &self.pull_requests.detail {
                view = view
                    .child(
                        column()
                            .px_3()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .child(format!("#{} {}", detail.number, detail.title)),
                            )
                            .child(label(format!(
                                "{}{} · {} → {}",
                                if detail.is_draft { "Draft · " } else { "" },
                                detail.state,
                                detail.head_ref_name,
                                detail.base_ref_name
                            ))),
                    )
                    .child(
                        row().px_3().gap_2().children(
                            [
                                ("overview", "Overview"),
                                ("checks", "Checks"),
                                ("diff", "Files changed"),
                            ]
                            .into_iter()
                            .map(|(tab, title)| {
                                control(SharedString::from(format!("pr-tab-{tab}")), title)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.pr_tab(tab, cx)),
                                    )
                            }),
                        ),
                    );
                if self.pull_requests.tab == "diff" {
                    view = view.child(
                        uniform_list(
                            "pr-diff-lines",
                            self.pull_requests.patch.len(),
                            cx.processor(Self::pr_diff_rows),
                        )
                        .flex_1()
                        .min_h_0()
                        .track_scroll(&self.pull_requests.diff_scroll),
                    );
                } else {
                    let mut body = column()
                        .id("pr-detail-content")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_3()
                        .gap_3();
                    if self.pull_requests.tab == "checks" {
                        body = body
                            .when(detail.status_check_rollup.is_empty(), |d| {
                                d.child(label(tr(cx, msg::native::no_checks_reported)))
                            })
                            .children(detail.status_check_rollup.iter().map(|check| {
                                column()
                                    .gap_1()
                                    .child(label(if check.name.is_empty() {
                                        check.context.clone()
                                    } else {
                                        check.name.clone()
                                    }))
                                    .child(label(format!(
                                        "{} {} {}",
                                        check.status, check.conclusion, check.state
                                    )))
                            }));
                    } else {
                        body = body
                            .children(
                                zuse_markdown::parse_full(&detail.body)
                                    .blocks
                                    .iter()
                                    .map(|b| markdown_block(&b.block)),
                            )
                            .child(label(format!(
                                "Review: {} · Merge: {}",
                                detail.review_decision, detail.mergeable
                            )))
                            .children(detail.activity_comments().map(|comment| {
                                column()
                                    .gap_1()
                                    .py_2()
                                    .border_t_1()
                                    .border_color(crate::palette::rgb(0x343434))
                                    .child(label(format!(
                                        "{} · {}",
                                        comment.author.login,
                                        if comment.submitted_at.is_empty() {
                                            &comment.created_at
                                        } else {
                                            &comment.submitted_at
                                        }
                                    )))
                                    .children(
                                        zuse_markdown::parse_full(&comment.body)
                                            .blocks
                                            .iter()
                                            .map(|b| markdown_block(&b.block)),
                                    )
                            }));
                    }
                    view = view.child(body);
                }
                view = view.child(
                    column()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .child(self.pull_requests.comment.clone())
                        .child(
                            control(
                                "pr-post-comment",
                                if self.pull_requests.posting.is_some() {
                                    "Posting…"
                                } else {
                                    "Post comment"
                                },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.post_pr_comment(cx))),
                        ),
                );
            }
        } else {
            view = view
                .child(
                    label(
                        self.pull_requests
                            .repository
                            .clone()
                            .unwrap_or_else(|| "Select a project".into()),
                    )
                    .px_3(),
                )
                .child(
                    row().px_3().gap_2().children(
                        [
                            (ChangeRequestFilter::Authored, "Authored"),
                            (ChangeRequestFilter::Reviewing, "Reviewing"),
                            (ChangeRequestFilter::All, "All"),
                        ]
                        .into_iter()
                        .map(|(filter, title)| {
                            control(SharedString::from(format!("pr-filter-{title}")), title)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.pull_requests.reads.is_empty() {
                                        return;
                                    }
                                    this.pull_requests.filter = filter;
                                    this.load_pr_page(false);
                                    cx.notify();
                                }))
                        }),
                    ),
                )
                .when(!busy && self.pull_requests.items.is_empty(), |d| {
                    d.child(label(tr(cx, msg::native::no_pull_requests_in_this_view)).p_3())
                })
                .child(
                    uniform_list(
                        "pr-board",
                        self.pull_requests.items.len(),
                        cx.processor(Self::pr_rows),
                    )
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.pull_requests.scroll),
                )
                .when(self.pull_requests.next.is_some(), |d| {
                    d.child(
                        control(
                            "pr-load-more",
                            tr(cx, msg::settings::diagnostics_pane_load_more),
                        )
                        .mx_3()
                        .mb_3()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.pull_requests.reads.is_empty() {
                                this.load_pr_page(true);
                            }
                            cx.notify();
                        })),
                    )
                });
        }
        view.into_any_element()
    }
}

fn reconcile_watch_error(
    shown: &mut Option<String>,
    from_watch: &mut Option<String>,
    frame: Option<String>,
) {
    match frame {
        Some(error) => {
            *shown = Some(error.clone());
            *from_watch = Some(error);
        }
        None => {
            if from_watch
                .take()
                .is_some_and(|error| shown.as_ref() == Some(&error))
            {
                *shown = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_recovered_watch_clears_only_its_own_error() {
        let (mut shown, mut from_watch) = (None, None);
        super::reconcile_watch_error(&mut shown, &mut from_watch, Some("offline".into()));
        assert_eq!(shown.as_deref(), Some("offline"));
        super::reconcile_watch_error(&mut shown, &mut from_watch, None);
        assert_eq!(shown, None);

        super::reconcile_watch_error(&mut shown, &mut from_watch, Some("offline".into()));
        shown = Some("Comment failed to post".into());
        super::reconcile_watch_error(&mut shown, &mut from_watch, None);
        assert_eq!(shown.as_deref(), Some("Comment failed to post"));
    }

    use super::*;
    use std::prelude::v1::test;

    fn item(number: u64) -> ChangeRequestListItem {
        serde_json::from_value(json!({
            "provider":"github","repository":"acme/app","number":number,
            "title":"Feature","url":format!("https://github.com/acme/app/pull/{number}"),
            "headRefOid":"a".repeat(40),"state":"open","isDraft":false,
            "additions":1,"deletions":0,"mergeability":"unknown",
            "createdAt":"2026-10-18T00:00:00Z","updatedAt":"2026-10-18T00:00:00Z"
        }))
        .unwrap()
    }
    fn detail(head: char) -> Value {
        json!({"url":item(1).url,"number":1,"headRefOid":head.to_string().repeat(40),"baseRefOid":"f".repeat(40)})
    }
    fn read_id(view: &WorkspaceView, kind: fn(&Read) -> bool) -> u64 {
        *view
            .pull_requests
            .reads
            .iter()
            .find(|(_, r)| kind(r))
            .unwrap()
            .0
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
                view.selected = Some(json!({"id":"chat","cwd":"/repo","deviceId":"host"}));
            })
            .unwrap();
        (window, rx)
    }
    #[gpui::test]
    fn pr_pages_and_pinned_diff_retire_old_scope_and_revision(cx: &mut TestAppContext) {
        let (window, mut requests) = fixture(cx);
        window
            .update(cx, |view, _, cx| {
                view.refresh_pull_requests(cx);
                let id = read_id(view, |r| matches!(r, Read::Repository));
                view.receive(json!({"id":id,"ok":"acme/old-name"}), cx);
                let id = read_id(view, |r| matches!(r, Read::Page(false)));
                view.receive(
                    json!({"id":id,"ok":{"items":[item(1)],"nextCursor":"next","totalCount":2}}),
                    cx,
                );
                assert_eq!(view.pull_requests.repository.as_deref(), Some("acme/app"));
                view.load_pr_page(true);
                let id = read_id(view, |r| matches!(r, Read::Page(true)));
                view.receive(json!({"id":id,"ok":{"items":[item(1),item(2)]}}), cx);
                assert_eq!(view.pull_requests.items.len(), 2);
                view.open_pr(item(1), cx);
                let id = read_id(view, |r| matches!(r, Read::Detail));
                view.receive(json!({"id":id,"ok":detail('a')}), cx);
                view.pr_tab("diff", cx);
                let stale = read_id(view, |r| matches!(r, Read::Diff));
                view.refresh_pr_detail();
                let id = read_id(view, |r| matches!(r, Read::Detail));
                view.receive(json!({"id":id,"ok":detail('b')}), cx);
                assert!(!view.pull_requests.reads.contains_key(&stale));
                view.receive(json!({"id":stale,"ok":"stale patch"}), cx);
                assert!(view.pull_requests.patch.is_empty());
                let current = read_id(view, |r| matches!(r, Read::Diff));
                view.receive(json!({"id":current,"ok":"+current"}), cx);
                assert_eq!(&*view.pull_requests.patch, &["+current"]);
                view.refresh_pr_detail();
                let stale = read_id(view, |r| matches!(r, Read::Detail));
                view.reset_pull_requests(cx);
                view.receive(json!({"id":stale,"ok":detail('a')}), cx);
                assert!(view.pull_requests.detail.is_none());
            })
            .unwrap();
        let mut diffs = vec![];
        while let Ok(frame) = requests.try_recv() {
            if frame["method"].is_string() {
                assert_eq!(frame["params"]["targetDeviceId"], "host");
            }
            if frame["method"] == methods::GET_CHANGE_REQUEST_DIFF {
                diffs.push(frame);
            }
        }
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0]["params"]["headRefOid"], "a".repeat(40));
        assert_eq!(diffs[1]["params"]["headRefOid"], "b".repeat(40));
        assert_eq!(diffs[1]["params"]["baseRefOid"], "f".repeat(40));
    }
    #[gpui::test]
    fn pr_comment_receipt_loss_keeps_draft_and_only_refreshes_discussion(cx: &mut TestAppContext) {
        let (window, mut requests) = fixture(cx);
        window
            .update(cx, |view, _, cx| {
                view.pull_requests.selected = Some(item(1).url);
                view.pull_requests.comment.update(cx, |input, cx| {
                    input.set_value("Original comment".into(), cx)
                });
            })
            .unwrap();
        let id = window
            .update(cx, |view, _, cx| {
                view.post_pr_comment(cx);
                view.post_pr_comment(cx);
                let id = view.pull_requests.posting.as_ref().unwrap().0;
                view.pull_requests
                    .comment
                    .update(cx, |input, cx| input.set_value("New draft".into(), cx));
                id
            })
            .unwrap();
        window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id":id,"err":"Connection lost"}), cx);
                assert_eq!(view.pull_requests.comment.read(cx).value(cx), "New draft");
                assert!(view.pull_requests.uncertain.contains(&item(1).url));
                assert!(view.pull_requests.posting.is_none());
                assert!(
                    view.pull_requests
                        .reads
                        .values()
                        .any(|r| matches!(r, Read::Detail))
                );
                view.receive(json!({"id":id,"ok":{"id":"late receipt"}}), cx);
                assert_eq!(view.pull_requests.comment.read(cx).value(cx), "New draft");
                view.reset_pull_requests(cx);
                assert_eq!(view.pull_requests.drafts[&item(1).url], "New draft");
            })
            .unwrap();
        let mut writes = 0;
        while let Ok(frame) = requests.try_recv() {
            if frame["method"] == methods::POST_CHANGE_REQUEST_COMMENT {
                writes += 1;
                assert_eq!(frame["params"]["body"], "Original comment");
            }
        }
        assert_eq!(writes, 1);
    }
}
