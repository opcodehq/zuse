//! The Summary tab: the title, branch/status/reviews/checks facts, then quiet
//! sections for the description, checks, feedback and folded agent tools.
use super::*;
use crate::localization::message as tr;
use zuse_i18n::messages as msg;

/// Width of the property rows' label column (icon and name).
pub(super) const PROPERTY_LABEL: f32 = 112.0;
const PROPERTY_HEIGHT: f32 = 30.0;
/// A description this long starts clamped behind Show more.
const DESCRIPTION_LONG_BYTES: usize = 360;
const DESCRIPTION_LONG_LINES: usize = 8;
const DESCRIPTION_CLAMP: f32 = 132.0;
/// Feedback rows shown before View all.
const FEEDBACK_PREVIEW: usize = 5;

/// What the header knows, from the loaded details or the board's row.
pub(super) struct HeaderFacts {
    pub title: String,
    pub title_reserve: f32,
    pub author: String,
    /// Already phrased, e.g. "updated 3 days ago".
    pub age: Option<String>,
    /// (shown, copied): the short command and the one that names the repo.
    pub checkout: Option<(String, String)>,
    /// (base, head) branch names.
    pub refs: Option<(String, String)>,
    /// (files, additions, deletions); files may be unknown for a preview.
    pub changes: Option<(Option<usize>, u64, u64)>,
}

/// How a merge from the header's menu goes through.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MergeMenu {
    Closed,
    /// Merge right away, once GitHub's requirements are met.
    Now,
    /// Arm auto-merge for when checks pass.
    WhenReady,
    /// Ask GitHub to merge before checks pass.
    Anyway,
}

/// The lifecycle state's glyph, tint and word.
pub(super) fn state_style(
    state: &str,
    draft: bool,
    theme: &Theme,
) -> (&'static str, gpui::Hsla, &'static str) {
    let state = if draft && state.eq_ignore_ascii_case("open") {
        "DRAFT"
    } else {
        state
    };
    match state.to_ascii_uppercase().as_str() {
        "MERGED" => (crate::icons::GIT_MERGE, theme.syntax.keyword, "Merged"),
        "CLOSED" => (crate::icons::PULL_REQUEST_CLOSED, theme.danger, "Closed"),
        "DRAFT" => (crate::icons::PULL_REQUEST_DRAFT, theme.text_muted, "Draft"),
        _ => (crate::icons::PULL_REQUEST, theme.success, "Open"),
    }
}

/// The lifecycle state as a tinted glyph, beside the number in the toolbar.
pub(super) fn state_mark(state: &str, draft: bool, theme: &Theme) -> AnyElement {
    let (glyph, color, label) = state_style(state, draft, theme);
    div()
        .id("pr-state")
        .debug_selector(|| "pr-state".into())
        .flex_none()
        .tip(label)
        .child(
            crate::icons::icon(glyph)
                .size(px(14.0))
                .flex_none()
                .text_color(color),
        )
        .into_any_element()
}

/// The page's headline, wrapping rather than truncating.
fn page_title(title: String, reserve: f32, theme: &Theme) -> gpui::Div {
    div().min_w_0().pr(px(reserve)).child(
        div()
            .id("pr-detail-title")
            .debug_selector(|| "pr-detail-title".into())
            .min_w_0()
            .text_size(crate::typography::ui_rems(20.0))
            .line_height(crate::typography::ui_rems(28.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_strong)
            .child(title),
    )
}

/// Title, byline with the checkout command, refs with the change totals,
/// then a hairline. Shared by the loaded page and its placeholder.
pub(super) fn detail_header(
    facts: HeaderFacts,
    theme: &Theme,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let HeaderFacts {
        title,
        title_reserve,
        author,
        age,
        checkout,
        refs,
        changes,
    } = facts;
    let muted_mono = |text: String| {
        div()
            .min_w_0()
            .truncate()
            .font_family(theme.font_mono.clone())
            .text_size(crate::typography::text_xs())
            .text_color(theme.text_muted)
            .child(text)
    };
    // Wraps the checkout command under the author when both don't fit.
    let byline = div()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_x(px(12.0))
        .gap_y(px(4.0))
        .child(
            div()
                .flex_grow(1.0)
                .max_w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(super::super::pull_request_media::avatar(
                    &author,
                    "pr-author".into(),
                    18.0,
                    theme,
                ))
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(if author.is_empty() {
                            "Deleted account".to_owned()
                        } else {
                            author
                        }),
                )
                .when_some(age, |el, age| {
                    el.child(div().flex_none().text_color(theme.text_faint).child("·"))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_muted)
                                .child(age),
                        )
                }),
        );
    let refs_line = (refs.is_some() || changes.is_some()).then(|| {
        div()
            .min_w_0()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .when_some(refs, |el, (base, head)| {
                        el.child(muted_mono(base).flex_none().max_w(gpui::relative(0.4)))
                            .child(
                                crate::icons::icon(crate::icons::ARROW_LEFT)
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(theme.text_faint),
                            )
                            .child(match checkout {
                                // The head branch copies its checkout command.
                                Some((shown, copied)) => muted_mono(head)
                                    .id("pr-copy-checkout")
                                    .debug_selector(|| "pr-copy-checkout".into())
                                    .role(gpui::Role::Button)
                                    .aria_label(crate::localization::message(
                                        cx,
                                        zuse_i18n::messages::native::copy_checkout,
                                    ))
                                    .tab_index(0)
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.text_color(theme.text).bg(theme.glass_hover())
                                    })
                                    .tip(format!("Copy {shown}"))
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copied.clone(),
                                        ));
                                    })
                                    .into_any_element(),
                                None => muted_mono(head).into_any_element(),
                            })
                    }),
            )
            .when_some(changes, |el, (files, additions, deletions)| {
                el.child(
                    div()
                        .id("pr-header-changes")
                        .debug_selector(|| "pr-header-changes".into())
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .when_some(files, |el, files| {
                            el.child(
                                crate::icons::icon(crate::icons::DOCUMENT)
                                    .size(px(13.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(format!(
                                "{files} {}",
                                if files == 1 { "file" } else { "files" }
                            ))
                        })
                        .child(super::code::counts(additions, deletions, 12.0, theme)),
                )
            })
    });
    div()
        .id("pr-detail-header")
        .debug_selector(|| "pr-detail-header".into())
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(8.0))
                .child(page_title(title, title_reserve, theme)),
        )
        .child(
            div()
                .id("pr-detail-meta")
                .debug_selector(|| "pr-detail-meta".into())
                .flex_none()
                .mt(px(6.0))
                .px(px(8.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .text_size(crate::typography::text_xs())
                .child(byline)
                .children(refs_line),
        )
}

/// One property: a muted icon and name in a fixed column, the value after.
/// Values wrap within their own column, so labels stay aligned.
pub(crate) fn property_row(
    id: &str,
    glyph: &'static str,
    label: &str,
    content: impl IntoElement,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let id = format!("pr-property-{id}");
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .min_w_0()
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .w(px(PROPERTY_LABEL))
                .h(px(PROPERTY_HEIGHT))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child(
                    crate::icons::icon(glyph)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().min_w_0().truncate().child(label.to_owned())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h(px(PROPERTY_HEIGHT))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .text_size(crate::typography::text_sm())
                .child(content),
        )
}

/// The checks rollup as one line: a terminal loader while anything runs,
/// otherwise the settled mark, then the counts.
fn checks_summary(detail: &ChangeRequestDetail, theme: &Theme) -> gpui::Div {
    let state = Facts::from_detail(detail).assess().ci.state;
    let statuses: Vec<String> = detail
        .status_check_rollup
        .iter()
        .map(check_status)
        .collect();
    let total = statuses.len();
    let passed = statuses
        .iter()
        .filter(|status| matches!(status.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED"))
        .count();
    let failed = statuses
        .iter()
        .filter(|status| check_failed(status))
        .count();
    let pending = total - passed - failed;
    let text = if total == 0 {
        match state {
            CiState::Unknown => "CI unavailable".to_owned(),
            CiState::NoChecks => "No checks reported".to_owned(),
            _ => status_style(ci_status(detail)).0,
        }
    } else if passed == total {
        format!("All {total} passed")
    } else {
        let mut parts = Vec::new();
        if failed > 0 {
            parts.push(format!("{failed} failing"));
        }
        if pending > 0 {
            parts.push(format!("{pending} running"));
        }
        if passed > 0 {
            parts.push(format!("{passed} passed"));
        }
        parts.join(", ")
    };
    let mark = if state == CiState::Pending {
        crate::loaders::retro_loader("pr-checks-running", 14.0, theme.loader_glyph())
    } else {
        let (glyph, color) = match state {
            CiState::Failed => (crate::icons::CLOSE_CIRCLE, theme.danger),
            CiState::Passed => (crate::icons::CHECK, theme.success),
            _ => (crate::icons::INFO_CIRCLE, theme.text_muted),
        };
        crate::icons::icon(glyph)
            .size(px(14.0))
            .flex_none()
            .text_color(color)
            .into_any_element()
    };
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(mark)
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(if state == CiState::Failed {
                    theme.danger
                } else {
                    theme.text
                })
                .child(text),
        )
}

/// The review decision in words and tone, if GitHub reported one.
fn review_decision(raw: &str, theme: &Theme) -> Option<(String, gpui::Hsla)> {
    match raw.to_ascii_uppercase().as_str() {
        "" => None,
        "APPROVED" => Some(("Approved".into(), theme.success)),
        "CHANGES_REQUESTED" => Some(("Changes requested".into(), theme.warning)),
        "REVIEW_REQUIRED" => Some(("Awaiting review".into(), theme.text_muted)),
        other => Some((humanize(other), theme.text_muted)),
    }
}

impl PullRequestDetailPage {
    pub(super) fn title_reserve(&self, state: &str, draft: bool) -> f32 {
        if !state.eq_ignore_ascii_case("open") {
            return 0.0;
        }
        let desired: f32 = if draft || self.updating_pr {
            210.0
        } else {
            160.0
        };
        desired.min((self.page_width.unwrap_or(500.0) - 40.0).max(0.0))
    }
    /// Merge, in the pane's header. Choosing a method is the confirmation;
    /// a blocked pull request can still arm auto-merge or try anyway.
    pub(super) fn merge_control(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use zuse_protocol::{ChangeRequestMergeMethod as Method, ChangeRequestUpdate as Update};
        if !detail.state.eq_ignore_ascii_case("open") {
            return None;
        }
        let busy = self.updating_pr;
        let draft = detail.is_draft;
        let mergeable = can_merge(detail);
        let label = if busy {
            "Updating…"
        } else if draft {
            "Ready for review"
        } else {
            "Merge"
        };
        let solid = !busy && (draft || mergeable);
        let tint = if solid {
            theme.on_solid
        } else {
            theme.text_muted
        };
        let button = widgets::action_button(
            theme,
            if solid {
                widgets::ActionTone::Solid
            } else {
                widgets::ActionTone::Filled
            },
        )
        .id("pr-primary-merge")
        .debug_selector(|| "pr-primary-merge".into())
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(if busy { -1 } else { 0 })
        .flex_none()
        .min_h(px(26.0))
        .px(px(10.0))
        .py_0()
        .child(
            crate::icons::icon(crate::icons::GIT_MERGE)
                .size(px(13.0))
                .text_color(tint),
        )
        .child(label)
        .when(!draft && !busy, |el| {
            el.child(
                crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                    .size(px(11.0))
                    .text_color(tint),
            )
        })
        .when(busy, |el| el.opacity(0.6).cursor_default())
        .when(!busy, |el| {
            el.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|page, _, _, cx| {
                    if page.detail.as_ref().is_some_and(|d| d.is_draft) {
                        page.update_pr(Update::MarkReady, cx);
                    } else {
                        page.more_open = false;
                        page.merge_menu = if page.merge_menu == MergeMenu::Closed {
                            MergeMenu::Now
                        } else {
                            MergeMenu::Closed
                        };
                        cx.notify();
                    }
                }))
        });
        let open = self.merge_menu != MergeMenu::Closed && !busy && !draft;
        let menu = open.then(|| {
            let assessment = Facts::from_detail(detail).assess();
            let can_defer = can_defer_merge(detail);
            let mode = if can_defer {
                self.merge_menu
            } else {
                MergeMenu::Now
            };
            let live = mergeable || mode != MergeMenu::Now;
            let waiting = !mergeable
                && mode == MergeMenu::Now
                && assessment.ci.state == CiState::Pending
                && assessment.blockers.is_empty();
            let heading = (!mergeable).then(|| match mode {
                MergeMenu::WhenReady => "Merge when checks pass",
                MergeMenu::Anyway => "Merge before checks pass",
                _ => merge_blocker(detail),
            });
            let methods: Vec<AnyElement> = [
                ("pr-merge-squash", "Squash and merge", Method::Squash),
                ("pr-merge-commit", "Create a merge commit", Method::Merge),
                ("pr-merge-rebase", "Rebase and merge", Method::Rebase),
            ]
            .into_iter()
            .map(|(id, label, method)| {
                if !live {
                    return div()
                        .id(id)
                        .debug_selector(move || id.to_owned())
                        .role(gpui::Role::MenuItem)
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_color(theme.text_faint)
                        .child(label)
                        .into_any_element();
                }
                crate::popover::menu_row(theme, false, label)
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .role(gpui::Role::MenuItem)
                    .child(label)
                    .on_click(cx.listener(move |page, _, _, cx| {
                        let update = if mode == MergeMenu::WhenReady {
                            Update::EnableAutoMerge { method }
                        } else {
                            Update::Merge { method }
                        };
                        page.update_pr(update, cx)
                    }))
                    .into_any_element()
            })
            .collect();
            let mut footer: Vec<AnyElement> = Vec::new();
            if detail.auto_merge_request.is_some() {
                footer.push(
                    crate::popover::menu_row(theme, false, "pr-merge-auto-off")
                        .id("pr-merge-auto-off")
                        .debug_selector(|| "pr-merge-auto-off".into())
                        .role(gpui::Role::MenuItem)
                        .child("Turn off auto-merge")
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.update_pr(Update::DisableAutoMerge, cx)
                        }))
                        .into_any_element(),
                );
            }
            if can_defer {
                let rows = [
                    detail.auto_merge_request.is_none().then_some((
                        "pr-merge-auto",
                        "Enable auto-merge",
                        MergeMenu::WhenReady,
                    )),
                    Some(("pr-merge-force", "Force merge", MergeMenu::Anyway)),
                ];
                for (id, label, target) in rows.into_iter().flatten() {
                    footer.push(
                        crate::popover::menu_row(theme, mode == target, label)
                            .id(id)
                            .debug_selector(move || id.to_owned())
                            .role(gpui::Role::MenuItem)
                            .aria_expanded(mode == target)
                            .justify_between()
                            .child(label)
                            .child(
                                crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                                    .size(px(11.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .on_click(cx.listener(move |page, _, _, cx| {
                                page.merge_menu = if page.merge_menu == target {
                                    MergeMenu::Now
                                } else {
                                    target
                                };
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
            gpui::deferred(
                crate::popover::popover_card(theme)
                    .id("pr-merge-methods")
                    .debug_selector(|| "pr-merge-methods".into())
                    .absolute()
                    .top_full()
                    .right_0()
                    .mt(px(4.0))
                    .w(px(224.0))
                    .occlude()
                    .on_mouse_down_out(cx.listener(|page, _, _, cx| {
                        page.merge_menu = MergeMenu::Closed;
                        cx.notify();
                    }))
                    .children(heading.map(|heading| {
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .when(waiting, |el| {
                                el.child(crate::loaders::retro_loader(
                                    "pr-merge-waiting",
                                    12.0,
                                    theme.loader_glyph(),
                                ))
                            })
                            .child(heading)
                    }))
                    .children(methods)
                    .when(!footer.is_empty(), |el| {
                        el.child(crate::popover::menu_separator()).children(footer)
                    }),
            )
        });
        Some(
            div()
                .relative()
                .flex_none()
                .child(button)
                .children(menu)
                .into_any_element(),
        )
    }

    pub(super) fn update_pr(
        &mut self,
        update: zuse_protocol::ChangeRequestUpdate,
        cx: &mut Context<Self>,
    ) {
        if self.updating_pr {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.update_notice =
                Some("Connect to the engine before updating this pull request.".into());
            cx.notify();
            return;
        };
        let is_edit = matches!(
            &update,
            zuse_protocol::ChangeRequestUpdate::Edit { .. }
                | zuse_protocol::ChangeRequestUpdate::AddReviewers { .. }
        );
        let mut params = serde_json::json!({"url": self.url, "update": update,
            "headRefOid": self.detail.as_ref().map(|detail| &detail.head_ref_oid).filter(|head| zuse_protocol::change_request_assessment::valid_head_oid(head))});
        if let Some(target) = &self.target {
            params["targetDeviceId"] = target.clone().into();
        }
        self.merge_menu = MergeMenu::Closed;
        self.more_open = false;
        self.update_notice = None;
        self.task = None;
        self.loading = false;
        self.error = None;
        // Both land on whatever the head becomes, so pin the one on screen.
        let method = if matches!(
            &update,
            zuse_protocol::ChangeRequestUpdate::Merge { .. }
                | zuse_protocol::ChangeRequestUpdate::EnableAutoMerge { .. }
        ) {
            methods::UPDATE_CHANGE_REQUEST_AT_HEAD
        } else {
            methods::UPDATE_CHANGE_REQUEST
        };
        let (cache, target, url) = (self.cache.clone(), self.target.clone(), self.url.clone());
        cache.borrow_mut().evict(&target, &url);
        self.updating_pr = true;
        // An admitted write and its cache retirement outlive the open page.
        cx.spawn(async move |this, cx| {
            let call = engine.client().call(method, params);
            let deadline = cx.background_executor().timer(Duration::from_secs(60));
            futures::pin_mut!(call, deadline);
            let result = match futures::future::select(call, deadline).await {
                futures::future::Either::Left((result, _)) => result.map_err(|error| {
                    format!(
                        "Couldn't confirm the update. {} Check GitHub before trying again.",
                        failure_reason(&error)
                    )
                }),
                futures::future::Either::Right(_) => {
                    Err("The update timed out. Check GitHub before trying again.".into())
                }
            };
            cache.borrow_mut().evict(&target, &url);
            let _ = this.update(cx, |page, cx| {
                page.updating_pr = false;
                if is_edit
                    && result
                        .as_ref()
                        .is_ok_and(|value| value["refusal"].is_null())
                {
                    page.edit_mode = None;
                }
                page.refresh(cx);
                page.update_notice = match result {
                    Ok(value) => value["refusal"].as_str().map(str::to_owned),
                    Err(error) => Some(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The board knows when the pull request last changed; a chat-opened
    /// one falls back to its newest activity.
    fn age(&self, detail: &ChangeRequestDetail) -> Option<String> {
        self.preview
            .as_ref()
            .filter(|preview| preview.url == self.url)
            .map(|preview| {
                format!(
                    "updated {}",
                    crate::pull_requests::relative_time(preview.updated_at, chrono::Utc::now())
                )
            })
            .or_else(|| {
                detail
                    .activity_comments()
                    .map(|comment| {
                        if comment.created_at.is_empty() {
                            &comment.submitted_at
                        } else {
                            &comment.created_at
                        }
                    })
                    .filter(|time| !time.is_empty())
                    .max()
                    .map(|time| format!("active {}", activity_time(time)))
            })
    }

    /// The header's facts for loaded details.
    pub(super) fn header_facts(&self, detail: &ChangeRequestDetail) -> HeaderFacts {
        HeaderFacts {
            title: detail.title.clone(),
            title_reserve: self.title_reserve(&detail.state, detail.is_draft),
            author: detail.author.login.clone(),
            age: self.age(detail),
            checkout: Some((
                format!("gh pr checkout {}", detail.number),
                handoff::checkout_command(detail, &self.url),
            )),
            refs: (!detail.base_ref_name.is_empty() || !detail.head_ref_name.is_empty())
                .then(|| (detail.base_ref_name.clone(), detail.head_ref_name.clone())),
            changes: Some((Some(detail.files.len()), detail.additions, detail.deletions)),
        }
    }

    /// The title, the branch/status/reviews/checks facts, then quiet sections for
    /// the description, checks, feedback and agent tools.
    pub(super) fn summary_body(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let header = div()
            .id("pr-detail-header")
            .debug_selector(|| "pr-detail-header".into())
            .min_w_0()
            .px(px(8.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(page_title(
                detail.title.clone(),
                self.title_reserve(&detail.state, detail.is_draft),
                theme,
            ))
            .child(
                div()
                    .id("pr-detail-meta")
                    .debug_selector(|| "pr-detail-meta".into())
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(crate::typography::text_sm())
                    .child(super::super::pull_request_media::avatar(
                        &detail.author.login,
                        "pr-author".into(),
                        18.0,
                        theme,
                    ))
                    .child(div().min_w_0().truncate().text_color(theme.text).child(
                        if detail.author.login.is_empty() {
                            "Deleted account".to_owned()
                        } else {
                            detail.author.login.clone()
                        },
                    ))
                    .when_some(self.age(detail), |el, age| {
                        el.child(div().flex_none().text_color(theme.text_faint).child("·"))
                            .child(div().flex_none().text_color(theme.text_muted).child(age))
                    }),
            );

        let copied = handoff::checkout_command(detail, &self.url);
        let shown = format!("gh pr checkout {}", detail.number);
        let branch = div()
            .id("pr-copy-checkout")
            .debug_selector(|| "pr-copy-checkout".into())
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::native::copy_checkout))
            .tab_index(0)
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .tip(format!("Copy {shown}"))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text)
                    .child(detail.head_ref_name.clone()),
            )
            .child(div().flex_none().text_color(theme.text_faint).child("›"))
            .child(
                div()
                    .flex_none()
                    .text_color(theme.text)
                    .child(detail.base_ref_name.clone()),
            )
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
            });

        // Reviewers in the order they first reviewed, the author excluded.
        let mut reviewers: Vec<&str> = Vec::new();
        for review in &detail.reviews {
            let login = review.author.login.as_str();
            if !login.is_empty()
                && !login.eq_ignore_ascii_case(&detail.author.login)
                && !reviewers.contains(&login)
            {
                reviewers.push(login);
            }
        }
        let (review_label, review_color) = review_decision(&detail.review_decision, theme)
            .unwrap_or_else(|| match detail.reviews.last() {
                Some(review) if !reviewers.is_empty() => {
                    (status_style(&review.state).0, theme.text_muted)
                }
                _ => ("No reviews".to_owned(), theme.text_muted),
            });
        let reviews_value = div()
            .id("pr-review-summary")
            .debug_selector(|| "pr-review-summary".into())
            .role(gpui::Role::Button)
            .tab_index(0)
            .aria_label(format!("Reviews, {review_label}"))
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .children(reviewers.iter().take(4).enumerate().map(|(index, login)| {
                div()
                    .id(SharedString::from(format!("pr-reviewer-{index}")))
                    .flex_none()
                    .tip(login.to_string())
                    .child(super::super::pull_request_media::avatar(
                        login,
                        format!("pr-reviewer-avatar-{index}").into(),
                        18.0,
                        theme,
                    ))
            }))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(review_color)
                    .child(review_label),
            )
            .on_click(cx.listener(|page, _, _, cx| page.select_tab(Tab::Activity, cx)));

        let branch = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(branch)
            .child(
                div()
                    .id("pr-header-changes")
                    .debug_selector(|| "pr-header-changes".into())
                    .flex_none()
                    .child(super::code::counts(
                        detail.additions,
                        detail.deletions,
                        12.0,
                        theme,
                    )),
            );

        let (state_glyph, state_color, state_label) =
            state_style(&detail.state, detail.is_draft, theme);
        let auto_merging =
            detail.state.eq_ignore_ascii_case("open") && detail.auto_merge_request.is_some();
        let status = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                crate::icons::icon(state_glyph)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(state_color),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text)
                    .child(if auto_merging {
                        format!("{state_label}, auto-merge on")
                    } else {
                        state_label.to_owned()
                    }),
            );

        let properties = div()
            .id("pr-properties")
            .debug_selector(|| "pr-properties".into())
            .mt(px(10.0))
            .px(px(8.0))
            .flex()
            .flex_col()
            .child(property_row(
                "branch",
                crate::icons::GIT_BRANCH,
                "Branch",
                branch,
                theme,
            ))
            .child(property_row(
                "status",
                crate::icons::PULL_REQUEST,
                "Status",
                status,
                theme,
            ))
            .child(property_row(
                "reviews",
                crate::icons::EYE,
                "Reviews",
                reviews_value,
                theme,
            ))
            .child(property_row(
                "checks",
                crate::icons::CHECK_CIRCLE,
                "Checks",
                checks_summary(detail, theme),
                theme,
            ));

        let collapsed = self.description_collapsed;
        let description_toggle = section_toggle(
            "pr-description-toggle",
            tr(cx, msg::projects::pr_pane_description),
            None,
            collapsed,
            theme,
        )
        .on_click(cx.listener(|page, _, _, cx| {
            page.description_collapsed = !page.description_collapsed;
            cx.notify();
        }));
        let long = detail.body.len() > DESCRIPTION_LONG_BYTES
            || detail.body.lines().count() > DESCRIPTION_LONG_LINES;
        let clamped = long && !self.description_expanded;
        let description = (!collapsed).then(|| {
            div()
                .flex()
                .flex_col()
                .pt(px(8.0))
                .child(
                    div()
                        .id("pr-description")
                        .debug_selector(|| "pr-description".into())
                        .relative()
                        .px(px(8.0))
                        .when(clamped, |el| {
                            el.max_h(px(DESCRIPTION_CLAMP)).overflow_hidden()
                        })
                        .when(detail.body.is_empty(), |el| {
                            el.text_color(theme.text_muted)
                                .child(tr(cx, msg::projects::pr_pane_no_description))
                        })
                        .when_some(
                            self.body.as_ref().filter(|_| !detail.body.is_empty()),
                            |el, body| {
                                el.child(rich_text(
                                    body,
                                    format!("{}description", self.selection_prefix),
                                    &self.url,
                                    theme,
                                    window,
                                    self.diagrams.clone(),
                                    cx.weak_entity(),
                                    true,
                                ))
                            },
                        )
                        .when(clamped, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .bottom_0()
                                    .h(px(48.0))
                                    .bg(gpui::linear_gradient(
                                        180.0,
                                        gpui::linear_color_stop(theme.panel_bg().opacity(0.0), 0.0),
                                        gpui::linear_color_stop(theme.panel_bg(), 1.0),
                                    )),
                            )
                        }),
                )
                .when(long, |el| {
                    el.child(
                        div()
                            .id("pr-description-more")
                            .debug_selector(|| "pr-description-more".into())
                            .role(gpui::Role::Button)
                            .tab_index(0)
                            .mt(px(6.0))
                            .px(px(8.0))
                            .text_size(crate::typography::text_sm())
                            .text_color(theme.text_muted)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.text))
                            .child(if clamped { "Show more" } else { "Show less" })
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.description_expanded = !page.description_expanded;
                                cx.notify();
                            })),
                    )
                })
        });

        let total = detail.status_check_rollup.len();
        let checks_toggle = section_toggle(
            "pr-checks-toggle",
            "Checks".to_owned(),
            Some(total),
            !self.checks_expanded,
            theme,
        )
        .on_click(cx.listener(|page, _, _, cx| {
            page.checks_expanded = !page.checks_expanded;
            cx.notify();
        }));
        let checks_list = self.checks_expanded.then(|| {
            let mut ordered: Vec<_> = detail.status_check_rollup.iter().collect();
            ordered.sort_by_key(|check| check_rank(&check_status(check)));
            div()
                .id("pr-checks-list")
                .debug_selector(|| "pr-checks-list".into())
                .flex()
                .flex_col()
                .when(ordered.is_empty(), |el| {
                    el.child(
                        div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .text_color(theme.text_muted)
                            .child(assessment_ci_empty_copy(detail)),
                    )
                })
                .children(
                    ordered
                        .into_iter()
                        .enumerate()
                        .map(|(index, check)| self.check_row(index, check, theme)),
                )
        });

        let feedback = feedback_items(detail);
        let feedback_count = feedback.len();
        let feedback_section =
            (feedback_count > 0).then(|| {
                let toggle = section_toggle(
                    "pr-feedback-toggle",
                    "Feedback".to_owned(),
                    Some(feedback_count),
                    self.feedback_collapsed,
                    theme,
                )
                .on_click(cx.listener(|page, _, _, cx| {
                    page.feedback_collapsed = !page.feedback_collapsed;
                    cx.notify();
                }));
                let rows =
                    (!self.feedback_collapsed).then(|| {
                        div()
                            .id("pr-review-preview")
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .children(feedback.iter().take(FEEDBACK_PREVIEW).map(
                                |(index, comment)| {
                                    self.feedback_row(*index, comment, theme, window, cx)
                                },
                            ))
                            .when(feedback_count > FEEDBACK_PREVIEW, |el| {
                                el.child(
                                    div()
                                        .id("pr-view-reviews")
                                        .debug_selector(|| "pr-view-reviews".into())
                                        .role(gpui::Role::Button)
                                        .tab_index(0)
                                        .mt(px(4.0))
                                        .px(px(8.0))
                                        .text_size(crate::typography::text_sm())
                                        .text_color(theme.text_muted)
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(theme.text))
                                        .child(format!("View all {feedback_count}"))
                                        .on_click(cx.listener(|page, _, _, cx| {
                                            page.select_tab(Tab::Activity, cx)
                                        })),
                                )
                            })
                    });
                div()
                    .flex()
                    .flex_col()
                    .child(section_gap())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(toggle)
                            .child(
                                div()
                                    .id("pr-add-all-feedback")
                                    .debug_selector(|| "pr-add-all-feedback".into())
                                    .role(gpui::Role::Button)
                                    .tab_index(0)
                                    .mr(px(8.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .text_size(crate::typography::text_sm())
                                    .text_color(theme.text_muted)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.text))
                                    .child(
                                        crate::icons::icon(crate::icons::PR_ADD_TO_CHAT)
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(theme.text_muted),
                                    )
                                    .child("Add all")
                                    .on_click(cx.listener(|page, _, window, cx| {
                                        if let Some(detail) = &page.detail {
                                            let context = all_feedback_context(detail, &page.url);
                                            window.dispatch_action(
                                                Box::new(AddPullRequestContext(context)),
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    )
                    .children(rows)
            });

        let agent_toggle = section_toggle(
            "pr-agent-toggle",
            "Agent".to_owned(),
            None,
            !self.agent_expanded,
            theme,
        )
        .on_click(cx.listener(|page, _, _, cx| {
            page.agent_expanded = !page.agent_expanded;
            cx.notify();
        }));
        let agent = self.agent_expanded.then(|| {
            div()
                .id("pr-agent")
                .px(px(8.0))
                .flex()
                .flex_col()
                .children(self.handoff_rows(detail, theme, cx))
                .when_some(self.watch_control.clone(), |el, control| {
                    el.child(property_row(
                        "auto-fix",
                        crate::icons::PR_REPAIR,
                        "Auto-fix CI",
                        control,
                        theme,
                    ))
                })
        });

        div()
            .id("pr-overview")
            .debug_selector(|| "pr-overview".into())
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(properties)
            .child(section_gap())
            .child(description_toggle)
            .children(description)
            .child(div().h(px(24.0)).flex_none())
            .child(checks_toggle)
            .children(checks_list)
            .children(feedback_section)
            .child(section_gap())
            .child(agent_toggle)
            .children(agent)
    }

    fn check_row(
        &self,
        index: usize,
        check: &zuse_protocol::ChangeRequestCheck,
        theme: &Theme,
    ) -> gpui::Stateful<gpui::Div> {
        let name = if check.name.is_empty() {
            &check.context
        } else {
            &check.name
        };
        let status = check_status(check);
        let (label, glyph, tone) = status_style(if status.is_empty() {
            "PENDING"
        } else {
            &status
        });
        let (glyph, color, label_color) = match tone {
            StatusTone::Positive => (crate::icons::CHECK, theme.success, theme.text_muted),
            StatusTone::Warning => (glyph, theme.warning, theme.text_muted),
            StatusTone::Negative => (crate::icons::CLOSE_CIRCLE, theme.danger, theme.danger),
            StatusTone::Neutral | StatusTone::Merged => (
                crate::icons::INFO_CIRCLE,
                theme.text_faint,
                theme.text_muted,
            ),
        };
        // Status integrations set these links, so only web pages open: never
        // file:, smb: or app schemes.
        let link = if check.details_url.is_empty() {
            &check.target_url
        } else {
            &check.details_url
        };
        let link = url::Url::parse(link)
            .ok()
            .filter(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
            .map(String::from)
            .unwrap_or_default();
        div()
            .id(SharedString::from(format!("pr-check-{index}")))
            .debug_selector(move || format!("pr-check-{index}"))
            .min_h(px(30.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(if tone == StatusTone::Warning {
                crate::loaders::retro_loader(
                    format!("pr-check-running-{index}"),
                    14.0,
                    theme.loader_glyph(),
                )
            } else {
                crate::icons::icon(glyph)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(color)
                    .into_any_element()
            })
            .child(
                div()
                    .id(SharedString::from(format!("pr-check-name-{index}")))
                    .debug_selector(move || format!("pr-check-name-{index}"))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::text_sm())
                    .child(name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::typography::text_xs())
                    .text_color(label_color)
                    .child(label),
            )
            // Always reserved, so the status column stays aligned.
            .child(
                div()
                    .size(px(14.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(!link.is_empty(), |el| {
                        el.child(
                            crate::icons::icon(crate::icons::ARROW_UP_RIGHT)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        )
                    }),
            )
            .when(!link.is_empty(), |el| {
                el.cursor_pointer()
                    .role(gpui::Role::Link)
                    .tab_index(0)
                    .aria_label(format!("Open {name}"))
                    .hover(|style| style.bg(theme.glass_hover()))
                    .on_click(move |_, _, cx| cx.open_url(&link))
            })
    }

    fn feedback_row(
        &self,
        index: usize,
        comment: &zuse_protocol::ChangeRequestComment,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = comment.id.clone();
        let link = comment.url.clone();
        let state = (!comment.state.is_empty()).then(|| status_style(&comment.state).0);
        let time = if comment.created_at.is_empty() {
            &comment.submitted_at
        } else {
            &comment.created_at
        };
        let location = self
            .detail
            .as_ref()
            .and_then(|detail| inline_thread(detail, index))
            .map(thread_location);
        div()
            .id(SharedString::from(format!("pr-review-preview-{index}")))
            .min_w_0()
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .min_w_0()
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(crate::typography::text_sm())
                    .child(super::super::pull_request_media::avatar(
                        &comment.author.login,
                        format!("pr-review-avatar-{index}").into(),
                        18.0,
                        theme,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(if comment.author.login.is_empty() {
                                "Deleted account".to_owned()
                            } else {
                                comment.author.login.clone()
                            }),
                    )
                    .children(state.map(|state| {
                        div()
                            .flex_none()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(state)
                    }))
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(activity_time(time)),
                    )
                    .child(div().flex_1())
                    .child(
                        action("pr-review-add", "Add to chat", theme).on_click(cx.listener(
                            move |page, _, window, cx| {
                                if let Some(context) = page.feedback_context(index, &id) {
                                    window.dispatch_action(
                                        Box::new(AddPullRequestContext(context)),
                                        cx,
                                    );
                                }
                            },
                        )),
                    )
                    .when(!link.is_empty(), |el| {
                        el.child(
                            action("pr-review-open", "Open on GitHub", theme)
                                .on_click(move |_, _, cx| cx.open_url(&link)),
                        )
                    }),
            )
            .children(
                location
                    .filter(|location| !location.is_empty())
                    .map(|location| {
                        div()
                            .px(px(12.0))
                            .pb(px(6.0))
                            .font_family(theme.font_mono.clone())
                            .text_size(crate::typography::text_xs())
                            .text_color(theme.text_muted)
                            .child(location)
                    }),
            )
            .children(
                self.activity_bodies
                    .get(index)
                    .filter(|_| !comment.body.is_empty())
                    .map(|body| {
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .border_t_1()
                            .border_color(theme.border)
                            .min_w_0()
                            .child(rich_text(
                                &super::super::pull_request_media::feedback_preview(body),
                                format!("{}feedback-{index}", self.selection_prefix),
                                &self.url,
                                theme,
                                window,
                                self.diagrams.clone(),
                                cx.weak_entity(),
                                false,
                            ))
                    }),
            )
            .when(!comment.body.is_empty(), |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("pr-feedback-activity-{index}")))
                        .px(px(12.0))
                        .pb(px(8.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.text))
                        .child("View in activity")
                        .on_click(
                            cx.listener(|page, _, _, cx| page.select_tab(super::Tab::Activity, cx)),
                        ),
                )
            })
    }
}

/// Failures first, then what is still running, then the settled rest.
fn check_rank(status: &str) -> u8 {
    if check_failed(status) {
        0
    } else if matches!(status, "SUCCESS") {
        2
    } else if matches!(status, "NEUTRAL" | "SKIPPED") {
        3
    } else {
        1
    }
}

fn section_gap() -> gpui::Div {
    div().h(px(16.0)).flex_none()
}

/// A quiet section heading that folds what follows it.
fn section_toggle(
    id: &'static str,
    label: String,
    count: Option<usize>,
    collapsed: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(gpui::Role::Button)
        .tab_index(0)
        .aria_label(label.clone())
        .aria_expanded(!collapsed)
        .h(px(26.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(crate::typography::text_sm())
        .text_color(theme.text_muted)
        .cursor_pointer()
        .hover(|style| style.text_color(theme.text))
        .child(label)
        .children(count.map(|count| div().child(count.to_string())))
        .child(
            crate::icons::icon(if collapsed {
                crate::icons::ALT_ARROW_RIGHT
            } else {
                crate::icons::ALT_ARROW_DOWN
            })
            .size(px(12.0))
            .flex_none()
            .text_color(theme.text_muted),
        )
}

pub(super) fn can_merge(detail: &ChangeRequestDetail) -> bool {
    let assessment = Facts::from_detail(detail).assess();
    detail.state.eq_ignore_ascii_case("open")
        && !detail.is_draft
        && detail.mergeable == "MERGEABLE"
        && zuse_protocol::change_request_assessment::valid_head_oid(&detail.head_ref_oid)
        && assessment.blockers.is_empty()
        && !matches!(
            assessment.ci.state,
            CiState::Pending | CiState::Failed | CiState::Unknown
        )
}

/// Auto-merge and Force merge need a head to pin and a tree GitHub could
/// merge; conflicts can't be waited out or forced through.
fn can_defer_merge(detail: &ChangeRequestDetail) -> bool {
    !can_merge(detail)
        && detail.state.eq_ignore_ascii_case("open")
        && !detail.is_draft
        && detail.mergeable != "CONFLICTING"
        && zuse_protocol::change_request_assessment::valid_head_oid(&detail.head_ref_oid)
}

/// Why Merge can't go straight through, for the top of its menu.
fn merge_blocker(detail: &ChangeRequestDetail) -> &'static str {
    let assessment = Facts::from_detail(detail).assess();
    if let Some(blocker) = assessment.blockers.first() {
        blocker.label()
    } else if assessment.ci.state == CiState::Pending {
        "Waiting on checks"
    } else {
        "Merge status unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use std::sync::Mutex;

    #[test]
    fn deferred_merges_need_a_pinned_head_and_no_conflicts() {
        let pending = ChangeRequestDetail {
            state: "OPEN".into(),
            mergeable: "MERGEABLE".into(),
            head_ref_oid: "a".repeat(40),
            status_check_rollup: vec![zuse_protocol::ChangeRequestCheck {
                name: "engine".into(),
                status: "IN_PROGRESS".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!can_merge(&pending));
        assert!(can_defer_merge(&pending));
        for blocked in [
            ChangeRequestDetail {
                mergeable: "CONFLICTING".into(),
                ..pending.clone()
            },
            ChangeRequestDetail {
                head_ref_oid: String::new(),
                ..pending.clone()
            },
            ChangeRequestDetail {
                is_draft: true,
                ..pending.clone()
            },
            ChangeRequestDetail {
                state: "MERGED".into(),
                ..pending.clone()
            },
        ] {
            assert!(!can_defer_merge(&blocked));
        }
    }

    #[gpui::test]
    fn pull_request_merge_outcome_survives_a_later_refresh(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let url = "https://github.com/acme/app/pull/1";
        let window = cx.add_window(|window, cx| {
            PullRequestDetailPage::new(
                fixture::state(cx, None),
                url.into(),
                None,
                Rc::new(RefCell::new(PullRequestCache::default())),
                None,
                window,
                cx,
            )
        });
        window
            .update(cx, |page, _, cx| {
                page.update_notice = Some("Merge refused".into());
                page.error = Some("Stale read error".into());
                page.accept_snapshot(
                    Ok(DetailSnapshot {
                        discussion_loaded: true,
                        detail: ChangeRequestDetail {
                            url: url.into(),
                            ..Default::default()
                        },
                        body: crate::markdown::parse_full(""),
                        activity: vec![],
                        fetched: Instant::now(),
                        diff: None,
                        diff_refresh_owed: false,
                    }),
                    cx,
                );
                assert_eq!(page.error, None);
                assert_eq!(page.update_notice.as_deref(), Some("Merge refused"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn pull_request_merge_keeps_its_receipt_and_retires_cache_after_closing(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _runtime = runtime.enter();
        fixture::init(cx);
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let rpc = ScriptedRpc::new({
            let release = release.clone();
            let calls = calls.clone();
            move |method, params| {
                let release = release.clone();
                let calls = calls.clone();
                async move {
                    assert_eq!(method, methods::UPDATE_CHANGE_REQUEST_AT_HEAD);
                    calls.lock().unwrap().push(params);
                    release.notified().await;
                    zuse_rpc::RpcReply::value(&serde_json::json!({"refusal": null}))
                }
            }
        });
        let url = "https://github.com/acme/app/pull/1";
        let snapshot = || DetailSnapshot {
            discussion_loaded: true,
            detail: ChangeRequestDetail {
                url: url.into(),
                head_ref_oid: "a".repeat(40),
                ..Default::default()
            },
            body: crate::markdown::parse_full(""),
            activity: vec![],
            fetched: Instant::now(),
            diff: None,
            diff_refresh_owed: false,
        };
        let cache = Rc::new(RefCell::new(PullRequestCache::default()));
        cache.borrow_mut().put(None, url.into(), snapshot());
        let window = cx.add_window(|window, cx| {
            PullRequestDetailPage::new(
                fixture::state(cx, Some(rpc.client())),
                url.into(),
                None,
                cache.clone(),
                None,
                window,
                cx,
            )
        });
        window
            .update(cx, |page, _, cx| {
                let update = zuse_protocol::ChangeRequestUpdate::Merge {
                    method: zuse_protocol::ChangeRequestMergeMethod::Squash,
                };
                page.update_pr(update.clone(), cx);
                page.update_pr(update, cx);
            })
            .unwrap();
        rpc.settle(cx, &runtime, |_| calls.lock().unwrap().len() == 1);
        assert!(cache.borrow_mut().get(&None, url).is_none());
        assert_eq!(calls.lock().unwrap()[0]["headRefOid"], "a".repeat(40));
        // A read from another view during the write must also be retired.
        cache.borrow_mut().put(None, url.into(), snapshot());
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        release.notify_one();
        rpc.settle(cx, &runtime, |_| {
            cache.borrow_mut().get(&None, url).is_none()
        });
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "closing never replays the write"
        );
    }
}
