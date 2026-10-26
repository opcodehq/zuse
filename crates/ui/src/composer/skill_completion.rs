//! Composer skill suggestions follow the desktop catalogue's names and compact rows.
use super::*;

pub(super) const ROW_HEIGHT: f32 = 32.;

pub(super) fn description(candidate: &InvocationCandidate) -> String {
    let mut text = candidate
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(hint) = &candidate.input_hint {
        let hint = hint.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            text.push_str(" · ");
        }
        text.push_str(&format!("<{hint}>"));
    }
    text
}

/// Prefer names, then descriptions; compact subsequences support abbreviated queries.
fn match_score(query: &str, value: &str) -> Option<(usize, usize)> {
    if value.starts_with(query) {
        return Some((0, 0));
    }
    if let Some(at) = value.find(query) {
        return Some((1, at));
    }
    let mut needle = query.chars();
    let mut next = needle.next()?;
    let mut first = None;
    let mut matched = 0;
    for (position, ch) in value.chars().enumerate() {
        if ch != next {
            continue;
        }
        let start = *first.get_or_insert(position);
        matched += 1;
        let Some(ch) = needle.next() else {
            let span = position - start + 1;
            return (matched * 10 >= span * 3).then_some((2, span - matched));
        };
        next = ch;
    }
    None
}

pub(super) fn filter(query: &str, candidates: &[InvocationCandidate]) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..candidates.len()).collect();
    }
    let mut ranked: Vec<_> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            let name = match_score(&query, &candidate.name.to_lowercase())
                .map(|(kind, gap)| (kind, 0, gap));
            let detail = match_score(&query, &description(candidate).to_lowercase())
                .map(|(kind, gap)| (kind, 1, gap));
            name.into_iter()
                .chain(detail)
                .min()
                .map(|score| (score, index))
        })
        .collect();
    ranked.sort_by_key(|&(score, index)| (score, index));
    ranked
        .into_iter()
        .take(50)
        .map(|(_, index)| index)
        .collect()
}

pub(super) fn row(
    theme: &Theme,
    candidate: &InvocationCandidate,
    selected: bool,
    index: usize,
) -> gpui::Stateful<gpui::Div> {
    let skill = candidate.invocation.prefix() == '$';
    let name = if skill {
        candidate.name.clone()
    } else {
        format!("/{}", candidate.name)
    };
    crate::popover::menu_row(theme, selected, format!("slash-result-{index}"))
        .id(("slash-result", index))
        .debug_selector(move || format!("slash-result-{index}"))
        .role(gpui::Role::ListBoxOption)
        .aria_selected(selected)
        .w_full()
        .h(px(ROW_HEIGHT))
        .min_h(px(ROW_HEIGHT))
        .max_h(px(ROW_HEIGHT))
        .flex_none()
        .overflow_hidden()
        .gap(px(12.))
        .rounded(px(8.))
        .when(skill, |row| {
            row.child(
                crate::icons::icon(crate::icons::SKILL)
                    .size(px(14.))
                    .flex_none()
                    .text_color(theme.accent.opacity(0.75)),
            )
        })
        .child(
            div()
                .min_w_0()
                .max_w(gpui::relative(0.55))
                .flex_none()
                .h(px(20.))
                .font_family(theme.font_mono.clone())
                .font_weight(gpui::FontWeight::NORMAL)
                .text_size(px(14.))
                .line_height(px(20.))
                .truncate()
                .line_clamp(1)
                .text_color(theme.text)
                .child(name),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .h(px(16.))
                .text_size(px(12.))
                .line_height(px(16.))
                .truncate()
                .line_clamp(1)
                .text_color(theme.text_muted)
                .child(description(candidate)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext};

    fn candidate(name: &str, description: &str) -> InvocationCandidate {
        InvocationCandidate {
            workspace_command: None,
            name: name.into(),
            description: description.into(),
            input_hint: None,
            invocation: zuse_protocol::invocation::Invocation::Skill {
                name: name.into(),
                path: format!("/project/.agents/skills/{name}/SKILL.md"),
                command: None,
            },
        }
    }
    #[test]
    fn skills_keep_their_names_and_descriptions_do_not_expose_source_paths() {
        let mut skill = zuse_protocol::invocation::Skill {
            name: "tools:office-hours".into(),
            path: "/project/.agents/skills/office/SKILL.md".into(),
            description: "Plan a project.\n\nCheck the assumptions.\tThen choose a direction."
                .into(),
            enabled: true,
            command: None,
        };
        let candidates = invocation_candidates(vec![], vec![skill.clone()]);
        assert_eq!(candidates[0].name, "tools:office-hours");
        assert_eq!(
            description(&candidates[0]),
            "Plan a project. Check the assumptions. Then choose a direction."
        );
        assert!(!description(&candidates[0]).contains("/project/"));
        skill.enabled = false;
        assert!(invocation_candidates(vec![], vec![skill]).is_empty());
    }
    #[test]
    fn search_matches_names_descriptions_and_abbreviations_with_a_result_limit() {
        let candidates = vec![
            candidate("office-hours", "Brainstorm the next project"),
            candidate("review", "Review source changes"),
        ];
        assert_eq!(filter("brainstorm", &candidates), vec![0]);
        assert_eq!(filter("ofhr", &candidates), vec![0]);
        assert_eq!(filter("review", &candidates), vec![1]);
        assert!(filter("no-such-skill", &candidates).is_empty());
        let many = (0..80)
            .map(|i| candidate(&format!("skill-{i}"), "Example"))
            .collect::<Vec<_>>();
        assert_eq!(filter("skill", &many).len(), 50);
        assert_eq!(filter("", &many).len(), 80);
    }
    struct Suggestion {
        width: f32,
    }
    impl Render for Suggestion {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).child(row(
                &Theme::dark(),
                &candidate(
                    "tools:office-hours",
                    &"Plan a project.\nCheck assumptions and choose a direction. ".repeat(12),
                ),
                true,
                0,
            ))
        }
    }
    #[gpui::test]
    fn long_multiline_skill_descriptions_stay_in_a_single_compact_row(cx: &mut TestAppContext) {
        for width in [300., 740.] {
            let (_, cx) = cx.add_window_view(|_, _| Suggestion { width });
            cx.update(|window, cx| window.draw(cx).clear());
            let bounds = cx.debug_bounds("slash-result-0").unwrap();
            assert_eq!(f32::from(bounds.size.height), ROW_HEIGHT);
            assert_eq!(f32::from(bounds.size.width), width);
        }
    }
    #[gpui::test]
    fn popup_renders_selected_and_unselected_skills_without_colliding_hover_handlers(
        cx: &mut TestAppContext,
    ) {
        let (_dir, window) = super::super::tests::composer_focus_window(cx);
        window
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("$", cx))
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |composer, _, cx| {
                composer.slash = SlashState {
                    token: invocation_token("$", 1, '$'),
                    context: "skill-test".into(),
                    skill: true,
                    filtered: vec![0, 1],
                    active: Some(0),
                    ..Default::default()
                };
                composer.slash_cache.insert(
                    "skill-test".into(),
                    vec![
                        candidate("office-hours", "Plan a project"),
                        candidate("review", "Review source changes"),
                    ],
                );
                composer.sync_mention_controls(cx);
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        window
            .update(cx, |composer, _, cx| composer.move_slash(1, cx))
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        window
            .read_with(cx, |composer, _| assert_eq!(composer.slash.active, Some(1)))
            .unwrap();
    }
    #[gpui::test]
    fn choosing_a_skill_keeps_its_identity_as_an_atomic_undoable_chip(cx: &mut TestAppContext) {
        let (_dir, window) = super::super::tests::composer_focus_window(cx);
        window
            .update(cx, |composer, window, cx| {
                let skill = candidate("tools:office-hours", "Plan a project.");
                let selected = skill.invocation.clone();
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("$office", cx));
                composer.slash = SlashState {
                    token: invocation_token("$office", 7, '$'),
                    context: "skill-test".into(),
                    skill: true,
                    filtered: vec![0],
                    active: Some(0),
                    ..Default::default()
                };
                composer
                    .slash_cache
                    .insert("skill-test".into(), vec![skill]);
                composer.accept_slash(cx);
                let raw = composer.input.read(cx).text();
                assert_eq!(
                    zuse_protocol::invocation::invocation_links(raw)[0].1,
                    selected
                );
                let projection = TextProjection::rich(raw, None, None);
                assert!(projection.display.contains("tools:office-hours"));
                assert!(composer.slash.token.is_none());
                composer.input.update(cx, |input, cx| {
                    input.undo(&Undo, window, cx);
                    assert_eq!(input.text(), "$office");
                });
            })
            .unwrap();
    }
}
