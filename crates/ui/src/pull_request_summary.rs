//! The Summary tab: a document-style header (title, author, checkout, refs
//! and change totals over a hairline), compact property rows, and the
//! description. Rows carry no card chrome; hairlines and spacing group them.
use super::*;

/// Width of the property rows' label column (icon and name).
pub(super) const PROPERTY_LABEL: f32 = 112.0;
const PROPERTY_HEIGHT: f32 = 32.0;

/// What the header knows, from the loaded details or the board's row.
pub(super) struct HeaderFacts {
    pub title: String,
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

/// The pull request's lifecycle state as a quiet bordered pill.
pub(super) fn state_pill(state: &str, draft: bool, theme: &Theme) -> AnyElement {
    let state = if draft { "DRAFT" } else { state };
    let (label, glyph, color) = match state.to_ascii_uppercase().as_str() {
        "MERGED" => ("Merged", crate::icons::GIT_MERGE, theme.syntax.keyword),
        "CLOSED" => ("Closed", crate::icons::PULL_REQUEST_CLOSED, theme.danger),
        "DRAFT" => ("Draft", crate::icons::PULL_REQUEST_DRAFT, theme.text_muted),
        _ => ("Open", crate::icons::PULL_REQUEST, theme.success),
    };
    div()
        .id("pr-state")
        .debug_selector(|| "pr-state".into())
        .flex_none()
        .h(px(22.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(theme.border_strong)
        .text_size(crate::typography::text_xs())
        .text_color(theme.text)
        .child(
            crate::icons::icon(glyph)
                .size(px(13.0))
                .flex_none()
                .text_color(color),
        )
        .child(label)
        .into_any_element()
}

/// Title, byline with the checkout command, refs with the change totals,
/// then a hairline. Shared by the loaded page and its placeholder.
pub(super) fn detail_header(facts: HeaderFacts, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let HeaderFacts {
        title,
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
        )
        .when_some(checkout, |el, (shown, copied)| {
            el.child(
                div()
                    .id("pr-copy-checkout")
                    .debug_selector(|| "pr-copy-checkout".into())
                    .role(gpui::Role::Button)
                    .aria_label("Copy checkout command")
                    .tab_index(0)
                    .flex_none()
                    .px(px(4.0))
                    .rounded(px(4.0))
                    .font_family(theme.font_mono.clone())
                    .text_size(crate::typography::text_xs())
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text).bg(theme.glass_hover()))
                    .tooltip(widgets::text_tooltip(format!("Copy {copied}")))
                    .child(shown)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                    }),
            )
        });
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
                            .child(muted_mono(head))
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
            // `page_header` keeps one line; a PR title wraps instead.
            div()
                .id("pr-detail-title")
                .debug_selector(|| "pr-detail-title".into())
                .px(px(8.0))
                .min_w_0()
                .text_size(crate::typography::text_xl())
                .line_height(crate::typography::ui_rems(26.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_strong)
                .child(title),
        )
        .child(
            div()
                .id("pr-detail-meta")
                .debug_selector(|| "pr-detail-meta".into())
                .flex_none()
                .mt(px(10.0))
                .px(px(8.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .text_size(crate::typography::text_sm())
                .child(byline)
                .children(refs_line),
        )
        .child(
            div()
                .debug_selector(|| "pr-detail-divider".into())
                .mt(px(16.0))
                .h(px(1.0))
                .flex_none()
                .bg(theme.border),
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

/// Glyph, color, and the one-line phrase for the checks rollup.
fn checks_summary(
    detail: &ChangeRequestDetail,
    theme: &Theme,
) -> (&'static str, gpui::Hsla, String) {
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
        format!("All {total} passing")
    } else {
        let mut parts = Vec::new();
        if failed > 0 {
            parts.push(format!("{failed} failing"));
        }
        if pending > 0 {
            parts.push(format!("{pending} running"));
        }
        parts.push(format!("{passed} of {total} passing"));
        parts.join(" · ")
    };
    let (glyph, color) = match state {
        CiState::Failed => (crate::icons::CLOSE_CIRCLE, theme.danger),
        CiState::Pending => (crate::icons::CLOCK_CIRCLE, theme.warning),
        CiState::Passed => (crate::icons::CHECK_CIRCLE, theme.success),
        _ => (crate::icons::INFO_CIRCLE, theme.text_muted),
    };
    (glyph, color, text)
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
    /// The header's facts for loaded details.
    pub(super) fn header_facts(&self, detail: &ChangeRequestDetail) -> HeaderFacts {
        // The board knows when the pull request last changed; a chat-opened
        // one falls back to its newest activity.
        let age = self
            .preview
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
            });
        HeaderFacts {
            title: detail.title.clone(),
            author: detail.author.login.clone(),
            age,
            checkout: Some((
                format!("gh pr checkout {}", detail.number),
                handoff::checkout_command(detail, &self.url),
            )),
            refs: (!detail.base_ref_name.is_empty() || !detail.head_ref_name.is_empty())
                .then(|| (detail.base_ref_name.clone(), detail.head_ref_name.clone())),
            changes: Some((Some(detail.files.len()), detail.additions, detail.deletions)),
        }
    }

    /// Property rows, checks, hand-off, and the description.
    pub(super) fn summary_body(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
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
        let decision = review_decision(&detail.review_decision, theme);
        let reviewers_row = property_row(
            "reviewers",
            crate::icons::EYE,
            "Reviewers",
            div()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .when(reviewers.is_empty(), |el| {
                    el.child(div().text_color(theme.text_muted).child("None"))
                })
                .children(reviewers.iter().enumerate().map(|(index, login)| {
                    div()
                        .id(SharedString::from(format!("pr-reviewer-{index}")))
                        .flex_none()
                        .tooltip(widgets::text_tooltip(login.to_string()))
                        .child(super::super::pull_request_media::avatar(
                            login,
                            format!("pr-reviewer-avatar-{index}").into(),
                            20.0,
                            theme,
                        ))
                }))
                .when_some(decision, |el, (label, color)| {
                    el.child(
                        div()
                            .ml(px(4.0))
                            .text_size(crate::typography::text_xs())
                            .text_color(color)
                            .child(label),
                    )
                }),
            theme,
        );

        let (glyph, color, phrase) = checks_summary(detail, theme);
        let total = Facts::from_detail(detail)
            .assess()
            .ci
            .total_count
            .max(detail.status_check_rollup.len() as u64);
        let expandable = !detail.status_check_rollup.is_empty() || self.checks_expanded;
        let toggle = div()
            .id("pr-checks-toggle")
            .debug_selector(|| "pr-checks-toggle".into())
            .role(gpui::Role::Button)
            .tab_index(0)
            .aria_label(format!("Checks, {total}, {phrase}"))
            .aria_expanded(self.checks_expanded)
            .h(px(26.0))
            .ml(px(-6.0))
            .px(px(6.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme.glass_hover()))
            .child(
                crate::icons::icon(glyph)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(color),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.text)
                    .child(phrase),
            )
            .when(expandable, |el| {
                el.child(
                    crate::icons::icon(if self.checks_expanded {
                        crate::icons::ALT_ARROW_DOWN
                    } else {
                        crate::icons::ALT_ARROW_RIGHT
                    })
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted),
                )
            })
            .on_click(cx.listener(|page, _, _, cx| {
                page.checks_expanded = !page.checks_expanded;
                cx.notify();
            }));
        let mut checks = div()
            .id("pr-checks-card")
            .debug_selector(|| "pr-checks-card".into())
            .min_w_0()
            .flex()
            .flex_col()
            .child(property_row(
                "checks",
                crate::icons::CHECKLIST,
                "Checks",
                toggle,
                theme,
            ));
        if self.checks_expanded {
            let mut list = div()
                .id("pr-checks-list")
                .debug_selector(|| "pr-checks-list".into())
                .ml(px(PROPERTY_LABEL + 12.0))
                .mb(px(6.0))
                .flex()
                .flex_col()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .overflow_hidden();
            if detail.status_check_rollup.is_empty() {
                list = list.child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_color(theme.text_muted)
                        .child(assessment_ci_empty_copy(detail)),
                );
            }
            for (index, check) in detail.status_check_rollup.iter().enumerate() {
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
                let tone = match tone {
                    StatusTone::Positive => theme.success,
                    StatusTone::Warning => theme.warning,
                    StatusTone::Negative => theme.danger,
                    StatusTone::Neutral | StatusTone::Merged => theme.text_muted,
                };
                // Status integrations set these links, so only web pages
                // open: never file:, smb: or app schemes.
                let link = if check.details_url.is_empty() {
                    &check.target_url
                } else {
                    &check.details_url
                };
                let link = url::Url::parse(link)
                    .ok()
                    .filter(|url| {
                        url.scheme() == "https"
                            && url.username().is_empty()
                            && url.password().is_none()
                    })
                    .map(String::from)
                    .unwrap_or_default();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("pr-check-{index}")))
                        .debug_selector(move || format!("pr-check-{index}"))
                        .min_h(px(32.0))
                        .px(px(12.0))
                        .py(px(4.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .when(index > 0, |el| el.border_t_1().border_color(theme.border))
                        .child(
                            crate::icons::icon(glyph)
                                .size(px(13.0))
                                .flex_none()
                                .text_color(tone),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("pr-check-name-{index}")))
                                .debug_selector(move || format!("pr-check-name-{index}"))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::text_xs())
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_muted)
                                .child(label),
                        )
                        .when(!link.is_empty(), |el| {
                            el.cursor_pointer()
                                .role(gpui::Role::Link)
                                .tab_index(0)
                                .aria_label(format!("Open {name}"))
                                .hover(|style| style.bg(theme.glass_hover()))
                                .on_click(move |_, _, cx| cx.open_url(&link))
                        }),
                );
            }
            checks = checks.child(list);
        }

        let assessment = Facts::from_detail(detail).assess();
        let status_row = property_row(
            "status",
            crate::icons::INFO_CIRCLE,
            "Status",
            div()
                .id("pr-assessment")
                .debug_selector(|| "pr-assessment".into())
                .min_w_0()
                .py(px(6.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_color(if assessment.blockers.is_empty() {
                            theme.text
                        } else {
                            theme.warning
                        })
                        .child(if assessment.blockers.is_empty() {
                            if assessment.missing.is_empty() {
                                "No blockers reported".to_owned()
                            } else {
                                "Assessment incomplete".to_owned()
                            }
                        } else {
                            assessment
                                .blockers
                                .iter()
                                .map(|blocker| blocker.label())
                                .collect::<Vec<_>>()
                                .join(" · ")
                        }),
                )
                .children(assessment.actions.first().map(|action| {
                    div()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(format!("Next: {}", action.label()))
                }))
                .children(assessment.missing.iter().map(|missing| {
                    div()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(missing.label())
                })),
            theme,
        );
        let commit_row = property_row(
            "commit",
            crate::icons::GIT_BRANCH,
            "Commit",
            div()
                .id("pr-head-commit")
                .min_w_0()
                .truncate()
                .tooltip(widgets::text_tooltip(if detail.head_ref_oid.is_empty() {
                    "Current commit unavailable".to_owned()
                } else {
                    detail.head_ref_oid.clone()
                }))
                .font_family(theme.font_mono.clone())
                .text_size(crate::typography::text_xs())
                .text_color(theme.text_muted)
                .child(if detail.head_ref_oid.is_empty() {
                    "Unavailable".to_owned()
                } else {
                    detail.head_ref_oid.chars().take(7).collect::<String>()
                }),
            theme,
        );

        let collapsed = self.description_collapsed;
        let description_toggle = div()
            .id("pr-description-toggle")
            .debug_selector(|| "pr-description-toggle".into())
            .role(gpui::Role::Button)
            .tab_index(0)
            .aria_label("Description")
            .aria_expanded(!collapsed)
            .mt(px(20.0))
            .h(px(26.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(6.0))
            .text_size(crate::typography::text_sm())
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(|style| style.text_color(theme.text))
            .child("Description")
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
            .on_click(cx.listener(|page, _, _, cx| {
                page.description_collapsed = !page.description_collapsed;
                cx.notify();
            }));
        let description = (!collapsed).then(|| {
            div()
                .id("pr-description")
                .debug_selector(|| "pr-description".into())
                .mt(px(4.0))
                .px(px(8.0))
                .when(detail.body.is_empty(), |el| {
                    el.text_color(theme.text_muted)
                        .child("No description provided.")
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
                            cx.weak_entity(),
                        ))
                    },
                )
        });

        div()
            .id("pr-overview")
            .debug_selector(|| "pr-overview".into())
            .mt(px(12.0))
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(8.0))
                    .flex()
                    .flex_col()
                    .child(reviewers_row)
                    .child(checks)
                    .child(status_row)
                    .child(commit_row)
                    .children(self.handoff_rows(detail, theme, cx)),
            )
            .child(description_toggle)
            .children(description)
    }
}
