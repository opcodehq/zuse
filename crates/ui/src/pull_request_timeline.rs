use super::*;
use zuse_protocol::ChangeRequestCommit;

fn commit_date(commit: &ChangeRequestCommit) -> &str {
    if commit.committed_date.is_empty() {
        &commit.authored_date
    } else {
        &commit.committed_date
    }
}

fn day_label(commit: &ChangeRequestCommit) -> String {
    chrono::DateTime::parse_from_rfc3339(commit_date(commit))
        .map(|date| {
            date.with_timezone(&chrono::Utc)
                .format("%b %-d, %Y")
                .to_string()
        })
        .unwrap_or_else(|_| "Date unavailable".into())
}

fn commit_url(pr_url: &str, oid: &str) -> Option<String> {
    if !zuse_protocol::change_request_assessment::valid_head_oid(oid) {
        return None;
    }
    let mut url = url::Url::parse(pr_url).ok()?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return None;
    }
    let segments: Vec<_> = url.path_segments()?.collect();
    if segments.len() != 4 || segments[2] != "pull" {
        return None;
    }
    let path = format!("/{}/{}/commit/{oid}", segments[0], segments[1]);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.into())
}

impl PullRequestDetailPage {
    pub(super) fn timeline_body(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let mut body = div()
            .id("pr-commit-timeline")
            .debug_selector(|| "pr-commit-timeline".into())
            .min_w_0()
            .flex()
            .flex_col()
            .child(summary::detail_header(self.header_facts(detail), theme, cx));
        let Some(commits) = &detail.commits else {
            return body.child(
                div()
                    .px(px(8.0))
                    .py(px(24.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .text_color(theme.text_muted)
                    .child("Commit history is unavailable. Refresh to try again.")
                    .child(
                        action("pr-timeline-refresh", "Refresh commits", theme)
                            .self_start()
                            .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                    ),
            );
        };
        let mut ordered: Vec<_> = commits.iter().collect();
        ordered
            .sort_by_key(|commit| chrono::DateTime::parse_from_rfc3339(commit_date(commit)).ok());
        body = body.child(
            div()
                .mt(px(24.0))
                .mb(px(8.0))
                .px(px(8.0))
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child("Commits"))
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .text_size(crate::typography::text_xs())
                        .child(commits.len().to_string()),
                ),
        );
        if ordered.is_empty() {
            return body.child(
                div()
                    .p(px(8.0))
                    .text_color(theme.text_muted)
                    .child("No commits to show."),
            );
        }
        let mut previous_day = None;
        for (index, commit) in ordered.iter().enumerate() {
            let day = day_label(commit);
            let first = index == 0 || day_label(ordered[index - 1]) != day;
            let last = index + 1 == ordered.len() || day_label(ordered[index + 1]) != day;
            if previous_day.as_ref() != Some(&day) {
                body = body.child(
                    div()
                        .px(px(8.0))
                        .pt(px(6.0))
                        .pb(px(10.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(day.clone()),
                );
                previous_day = Some(day);
            }
            let author = commit.authors.first();
            let login = author
                .and_then(|author| author.login.as_deref())
                .filter(|login| !login.is_empty());
            let author_name = login
                .or_else(|| author.and_then(|author| author.name.as_deref()))
                .filter(|name| !name.is_empty())
                .unwrap_or("Unknown author");
            let title = if commit.message_headline.trim().is_empty() {
                "Untitled commit"
            } else {
                commit.message_headline.trim()
            };
            let link = commit_url(&self.url, &commit.oid);
            let short_oid: String = commit.oid.chars().take(7).collect();
            let oid = commit.oid.clone();
            let select_oid = commit.oid.clone();
            let date = commit_date(commit).to_owned();
            body = body.child(
                crate::settings::widgets::card_row(theme, first)
                    .id(SharedString::from(format!("pr-commit-{index}")))
                    .debug_selector(|| "pr-commit-row".into())
                    .min_w_0()
                    .px(px(12.0))
                    .py(px(10.0))
                    .bg(crate::settings::widgets::block_fill(theme))
                    .when(first, |el| el.rounded_t(px(12.0)))
                    .when(last, |el| el.rounded_b(px(12.0)))
                    .flex_nowrap()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(16.0))
                            .h(px(20.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(
                                crate::icons::icon(crate::icons::GIT_BRANCH)
                                    .size(px(16.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("pr-commit-title-{index}")))
                                    .debug_selector(move || format!("pr-commit-title-{index}"))
                                    .h(px(20.0))
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_strong)
                                    .cursor_pointer()
                                    .role(gpui::Role::Button)
                                    .tab_index(0)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.select_commit(Some(select_oid.clone()), cx)
                                    }))
                                    .tip(title.to_owned())
                                    .child(title.to_owned()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("pr-commit-meta-{index}")))
                                    .debug_selector(move || format!("pr-commit-meta-{index}"))
                                    .h(px(18.0))
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .text_size(crate::typography::text_xs())
                                    .text_color(theme.text_muted)
                                    .when_some(login, |el, login| {
                                        el.child(crate::pull_request_media::avatar(
                                            login,
                                            format!("pr-commit-author-{index}").into(),
                                            14.0,
                                            theme,
                                        ))
                                    })
                                    .child(div().min_w_0().truncate().child(author_name.to_owned()))
                                    .child(
                                        div().flex_none().text_color(theme.text_faint).child("·"),
                                    )
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "pr-commit-date-{index}"
                                            )))
                                            .flex_none()
                                            .tip(date.clone())
                                            .child(activity_time(&date)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "pr-copy-commit-{index}"
                                            )))
                                            .debug_selector(move || {
                                                format!("pr-copy-commit-{index}")
                                            })
                                            .role(gpui::Role::Button)
                                            .aria_label(format!("Copy commit {}", commit.oid))
                                            .tab_index(0)
                                            .flex_none()
                                            .rounded(px(4.0))
                                            .px(px(4.0))
                                            .h(px(20.0))
                                            .flex()
                                            .items_center()
                                            .text_size(crate::typography::text_xs())
                                            .font_family(theme.font_mono.clone())
                                            .text_color(theme.text_muted)
                                            .cursor_pointer()
                                            .tip("Copy commit SHA")
                                            .hover(|style| {
                                                style.bg(theme.glass_hover()).text_color(theme.text)
                                            })
                                            .child(short_oid)
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(oid.clone()),
                                                );
                                            }),
                                    )
                                    .when_some(link, |el, link| {
                                        el.child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "pr-open-commit-{index}"
                                                )))
                                                .debug_selector(move || {
                                                    format!("pr-open-commit-{index}")
                                                })
                                                .role(gpui::Role::Link)
                                                .aria_label("Open commit on GitHub")
                                                .tab_index(0)
                                                .size(px(20.0))
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .tip("Open commit on GitHub")
                                                .child(
                                                    crate::icons::icon(
                                                        crate::icons::ARROW_UP_RIGHT,
                                                    )
                                                    .size(px(12.0))
                                                    .text_color(theme.text_muted),
                                                )
                                                .on_click(move |_, _, cx| cx.open_url(&link)),
                                        )
                                    }),
                            )
                            .when_some(
                                commit.additions.zip(commit.deletions),
                                |el, (additions, deletions)| {
                                    el.child(
                                        div()
                                            .debug_selector(move || {
                                                format!("pr-commit-counts-{index}")
                                            })
                                            .h(px(18.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .text_size(crate::typography::text_xs())
                                            .child(
                                                div()
                                                    .text_color(theme.success)
                                                    .child(format!("+{additions}")),
                                            )
                                            .child(
                                                div()
                                                    .text_color(theme.danger)
                                                    .child(format!("−{deletions}")),
                                            ),
                                    )
                                },
                            ),
                    ),
            );
        }
        if commits.len() >= 100 {
            let url = format!("{}/commits", self.url.trim_end_matches('/'));
            body = body.child(
                action("pr-timeline-more", "View full history on GitHub", theme)
                    .mt(px(12.0))
                    .self_start()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_links_use_the_pull_request_repository_and_valid_sha() {
        let oid = "a".repeat(40);
        assert_eq!(
            commit_url("https://github.com/acme/project/pull/5", &oid),
            Some(format!("https://github.com/acme/project/commit/{oid}"))
        );
        assert!(commit_url("https://github.com/acme/project/pull/5", "../settings").is_none());
        assert!(commit_url("https://example.com/acme/project/pull/5", &oid).is_none());
    }

    #[gpui::test]
    fn timeline_fits_narrow_panes_and_copies_the_full_commit_sha(cx: &mut gpui::TestAppContext) {
        use crate::pull_request_test_support as fixture;
        fixture::init(cx);
        let oid = "a".repeat(40);
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = fixture::state(cx, None);
            let mut page = PullRequestDetailPage::new(
                state,
                "https://github.com/acme/project/pull/5".into(),
                None,
                Default::default(),
                None,
                window,
                cx,
            );
            page.loading = false;
            page.detail = Some(ChangeRequestDetail {
                title: "Improve pull request navigation".into(),
                state: "OPEN".into(),
                commits: Some(vec![ChangeRequestCommit {
                    oid: oid.clone(),
                    message_headline:
                        "A long commit title that must leave room for the SHA and the external link"
                            .into(),
                    committed_date: "2026-11-13T09:00:00Z".into(),
                    additions: Some(12),
                    deletions: Some(3),
                    authors: vec![zuse_protocol::ChangeRequestCommitAuthor {
                        name: Some("Contributor without a GitHub account".into()),
                        login: None,
                    }],
                    ..Default::default()
                }]),
                ..Default::default()
            });
            page.tab = Tab::Timeline;
            page
        });
        for width in [240.0, 360.0, 640.0, 900.0] {
            cx.simulate_resize(gpui::size(px(width), px(800.0)));
            cx.run_until_parked();
            let title = cx.debug_bounds("pr-commit-title-0").unwrap();
            let meta = cx.debug_bounds("pr-commit-meta-0").unwrap();
            let copy = cx.debug_bounds("pr-copy-commit-0").unwrap();
            let open = cx.debug_bounds("pr-open-commit-0").unwrap();
            let counts = cx.debug_bounds("pr-commit-counts-0").unwrap();
            assert_eq!(title.left(), meta.left());
            assert!(title.left() >= px(0.0) && title.right() <= copy.left());
            assert!(copy.right() <= open.left() && open.right() <= px(width));
            assert!(meta.right() <= counts.left() && counts.right() <= px(width));
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            assert!(nav.left() >= px(0.0) && nav.right() <= px(width));
        }
        let copy = cx.debug_bounds("pr-copy-commit-0").unwrap();
        cx.simulate_mouse_down(
            copy.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            copy.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(oid.as_str())
            )
        });
        let title = cx.debug_bounds("pr-commit-title-0").unwrap();
        cx.simulate_mouse_down(
            title.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            title.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.tab == Tab::Code);
            assert_eq!(page.selected_commit.as_deref(), Some(oid.as_str()));
        });
        assert!(cx.debug_bounds("pr-selected-commit").is_some());
        let all = cx.debug_bounds("pr-all-changes").unwrap();
        cx.simulate_mouse_down(
            all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        page.read_with(cx, |page, _| assert!(page.selected_commit.is_none()));
        page.update(cx, |page, cx| {
            page.tab = Tab::Timeline;
            page.detail.as_mut().unwrap().commits = None;
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("pr-timeline-refresh").is_some());
    }
}
