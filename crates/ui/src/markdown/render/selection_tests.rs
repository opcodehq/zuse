use super::*;
use super::{
    code_block::{FenceLabel, render_code_block_source},
    selection_paint::REGISTRY,
    text::text_element,
};
use crate::markdown::parser::{InlineRun, InlineStyle, parse_full};
use gpui::TestAppContext;
use gpui::{Context, Render, Window, div, point, prelude::*, px, size};

#[derive(Default)]
struct CodeSelectionHarness {
    occluded: bool,
}

impl Render for CodeSelectionHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let opts = RenderOptions::settled("code-selection-test".into());
        let plain = |text: &str| {
            vec![InlineRun {
                text: text.into(),
                style: InlineStyle::default(),
            }]
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(selection_frame_reset())
            .child(text_element(
                &plain("before"),
                MD_TEXT_SIZE,
                MD_LINE_HEIGHT,
                false,
                0,
                0,
                &opts,
                &theme,
            ))
            .child(render_code_block_source(
                FenceLabel::default(),
                "selectable\n\nsecond",
                1,
                1,
                &opts,
                &theme,
                None,
            ))
            .child(text_element(
                &plain("after"),
                MD_TEXT_SIZE,
                MD_LINE_HEIGHT,
                false,
                2,
                2,
                &opts,
                &theme,
            ))
            .when(self.occluded, |root| {
                root.child(div().absolute().inset_0().occlude())
            })
    }
}

struct MarkdownSelectionHarness;

impl Render for MarkdownSelectionHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let source = "# Heading\n\nBefore **bold** and *italic* with `code`.\n\n\
            | Agent | Branch | Target |\n\
            | --- | --- | --- |\n\
            | linux | `lab-linux` | Linux jobs, ten minutes or less. This long target wraps across several visual lines in its cell. |\n\
            | macos | `lab-macos` | macOS jobs |\n\n\
            > A **quoted** paragraph.\n\n\
            - A list with `inline code`.\n\n\
            After the table.";
        div()
            .size_full()
            .child(selection_frame_reset())
            .child(render_tree(
                &parse_full(source),
                &RenderOptions::settled("markdown-selection-test".into()),
                &theme,
                window,
                &|_, _| None,
            ))
    }
}

fn selection_key_for_text(text: &str) -> String {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .find(|entry| entry.text.as_ref() == text)
            .unwrap_or_else(|| panic!("missing painted text: {text}"))
            .key
            .to_string()
    })
}

fn selection_position(key: &str, ix: usize) -> gpui::Point<gpui::Pixels> {
    let (_, layout, offsets) = selection_test_snapshot(key);
    let ix = offsets.as_ref().map_or(ix, |map| map.displayed(ix));
    layout.position_for_index(ix).expect("text position")
        + point(px(0.1), layout.line_height() / 2.0)
}

#[gpui::test]
fn markdown_drag_tracks_each_table_column_and_wrapped_cell(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| MarkdownSelectionHarness);
    cx.simulate_resize(size(px(560.0), px(800.0)));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });

    let target = "Linux jobs, ten minutes or less. This long target wraps across several visual lines in its cell.";
    let target_key = selection_key_for_text(target);
    assert!(selection_test_bounds(&target_key).size.height > px(MD_LINE_HEIGHT));
    // Every column must track the pointer, including later visual lines
    // in a wrapped cell, and a drag must contract and reverse normally.
    for text in ["linux", "lab-linux", target, "lab-macos", "macOS jobs"] {
        let key = selection_key_for_text(text);
        let start = selection_position(&key, 1);
        let end = selection_position(&key, text.len() - 1);
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: start,
            click_count: 1,
            ..Default::default()
        });
        for (position, expected) in [
            (end, &text[1..text.len() - 1]),
            (start, ""),
            (selection_position(&key, 0), &text[..1]),
        ] {
            cx.simulate_event(gpui::MouseMoveEvent {
                position,
                pressed_button: Some(gpui::MouseButton::Left),
                ..Default::default()
            });
            assert_eq!(
                super::super::selection::selected_text().unwrap_or_default(),
                expected,
                "drag inside {text}"
            );
        }
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position: selection_position(&key, 0),
            ..Default::default()
        });
        super::super::selection::clear_if_owner(&key);
    }
}

#[gpui::test]
fn markdown_drag_clamps_in_cell_padding_and_outside_paragraphs(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| MarkdownSelectionHarness);
    cx.simulate_resize(size(px(560.0), px(800.0)));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    for text in [
        "linux",
        "lab-linux",
        "lab-macos",
        "macOS jobs",
        "A quoted paragraph.",
    ] {
        let key = selection_key_for_text(text);
        let bounds = selection_test_bounds(&key);
        // Padding belongs to the nearest cell, even if an earlier column
        // has text at the same y. Each side clamps to that cell's endpoint.
        for (position, expected) in [
            (
                point(
                    bounds.right() + px(4.0),
                    selection_position(&key, text.len()).y,
                ),
                &text[1..],
            ),
            (
                point(bounds.left() - px(4.0), selection_position(&key, 0).y),
                &text[..1],
            ),
        ] {
            cx.simulate_event(gpui::MouseDownEvent {
                button: gpui::MouseButton::Left,
                position: selection_position(&key, 1),
                click_count: 1,
                ..Default::default()
            });
            cx.simulate_event(gpui::MouseMoveEvent {
                position,
                pressed_button: Some(gpui::MouseButton::Left),
                ..Default::default()
            });
            let selected = super::super::selection::selected_text();
            super::super::selection::end_active_drag();
            super::super::selection::clear_if_owner(&key);
            assert_eq!(selected.as_deref(), Some(expected), "padding beside {text}");
        }
    }
    // A far-right drag on a short heading must stay on that line instead
    // of jumping to a wider paragraph below (Euclidean nearest is wrong).
    let key = selection_key_for_text("Heading");
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: selection_position(&key, 0),
        click_count: 1,
        ..Default::default()
    });
    cx.simulate_event(gpui::MouseMoveEvent {
        position: selection_position(&key, 0) + point(px(1000.0), px(0.0)),
        pressed_button: Some(gpui::MouseButton::Left),
        ..Default::default()
    });
    let selected = super::super::selection::selected_text();
    super::super::selection::end_active_drag();
    super::super::selection::clear_if_owner(&key);
    assert_eq!(selected.as_deref(), Some("Heading"));
}

#[gpui::test]
fn markdown_drag_crosses_cells_and_formatted_blocks_in_document_order(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| MarkdownSelectionHarness);
    cx.simulate_resize(size(px(560.0), px(800.0)));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let branch_key = selection_key_for_text("lab-linux");
    let target = "Linux jobs, ten minutes or less. This long target wraps across several visual lines in its cell.";
    let target_key = selection_key_for_text(target);
    let macos_key = selection_key_for_text("lab-macos");
    let before_key = selection_key_for_text("Before bold and italic with code.");
    let after_key = selection_key_for_text("After the table.");
    for (anchor_key, anchor_ix, head_key, head_ix, expected) in [
        (
            &branch_key,
            4,
            &target_key,
            10,
            "linux\nLinux jobs".to_string(),
        ),
        (
            &target_key,
            10,
            &branch_key,
            4,
            "linux\nLinux jobs".to_string(),
        ),
        (
            &branch_key,
            4,
            &macos_key,
            3,
            format!("linux\n{target}\nmacos\nlab"),
        ),
        (
            &before_key,
            0,
            &after_key,
            16,
            format!(
                "Before bold and italic with code.\nAgent\nBranch\nTarget\nlinux\nlab-linux\n{target}\nmacos\nlab-macos\nmacOS jobs\nA quoted paragraph.\nA list with inline code.\nAfter the table."
            ),
        ),
    ] {
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: selection_position(anchor_key, anchor_ix),
            click_count: 1,
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseMoveEvent {
            position: selection_position(head_key, head_ix),
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        let selected = super::super::selection::selected_text();
        super::super::selection::end_active_drag();
        super::super::selection::clear_if_owner(anchor_key);
        assert_eq!(selected.as_deref(), Some(expected.as_str()));
    }
}

/// Two transcripts painted side by side (main chat + side chat or subagent
/// tab), each opening with its own `selection_frame_reset()`. A side
/// chat's forked history repeats its parent's row keys: `prefixes` equal.
struct TwoPaneSelectionHarness {
    prefixes: [&'static str; 2],
    second_pane: bool,
}

impl TwoPaneSelectionHarness {
    fn new(prefixes: [&'static str; 2]) -> Self {
        Self {
            prefixes,
            second_pane: true,
        }
    }
}

impl Render for TwoPaneSelectionHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let pane = |prefix: &str, surface: u64| {
            let opts = RenderOptions::settled(prefix.to_string().into());
            div()
                .w(px(320.0))
                .h_full()
                .child(selection_frame_reset_for(surface))
                .child(text_element(
                    &[InlineRun {
                        text: "alpha beta gamma delta".into(),
                        style: InlineStyle::default(),
                    }],
                    MD_TEXT_SIZE,
                    MD_LINE_HEIGHT,
                    false,
                    0,
                    0,
                    &opts,
                    &theme,
                ))
        };
        div()
            .size_full()
            .flex()
            .child(pane(self.prefixes[0], 1))
            .when(self.second_pane, |el| el.child(pane(self.prefixes[1], 2)))
    }
}

#[gpui::test]
fn dragging_selects_text_in_every_pane_of_a_two_transcript_layout(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| TwoPaneSelectionHarness::new(["pane-a", "pane-b"]));
    cx.simulate_resize(size(px(640.0), px(240.0)));
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    // Each pane is 320px wide; drag across the start of its text. The
    // last-painted pane is the control: it always worked.
    for (name, left) in [("pane-b", 320.0), ("pane-a", 0.0)] {
        let start = point(px(left + 1.0), px(9.0));
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: start,
            click_count: 1,
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseMoveEvent {
            position: start + point(px(60.0), px(0.0)),
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        let selected = super::super::selection::selected_text();
        super::super::selection::end_active_drag();
        super::super::selection::clear_if_owner(&format!("{name}:0"));
        assert!(
            selected.is_some_and(|text| !text.is_empty()),
            "a drag in {name} must select text"
        );
    }
}

#[gpui::test]
fn a_forked_pane_sharing_keys_keeps_its_selection_to_itself(cx: &mut TestAppContext) {
    use super::super::selection;
    let _selection = selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| TwoPaneSelectionHarness::new(["fork", "fork"]));
    cx.simulate_resize(size(px(640.0), px(240.0)));
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    // Both panes paint `fork:0`. Whichever pane the drag starts in, it
    // must select there (the other pane's same-key listener must not
    // clear it) and wash only there.
    for (surface, other, left) in [(1, 2, 0.0), (2, 1, 320.0)] {
        let start = point(px(left + 1.0), px(9.0));
        let end = start + point(px(60.0), px(0.0));
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: start,
            click_count: 1,
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseMoveEvent {
            position: end,
            pressed_button: Some(gpui::MouseButton::Left),
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position: end,
            ..Default::default()
        });
        let selected = selection::selected_text();
        let washed = selection::wash_range(surface, "fork:0");
        let leaked = selection::wash_range(other, "fork:0");
        selection::clear_if_owner("fork:0");
        let selected = selected.unwrap_or_default();
        assert!(
            "alpha beta gamma delta".starts_with(&selected) && !selected.is_empty(),
            "a drag in surface {surface} copies its own text, got {selected:?}"
        );
        assert_eq!(washed, Some(0..selected.len()), "surface {surface} washes");
        assert_eq!(leaked, None, "surface {other} stays unwashed");
    }
}

#[gpui::test]
fn a_closed_pane_leaves_no_registry_entries_behind(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (view, cx) = cx.add_window_view(|_, _| TwoPaneSelectionHarness::new(["pane-a", "pane-b"]));
    cx.simulate_resize(size(px(640.0), px(240.0)));
    let entries = |surface: u64| {
        REGISTRY.with(|r| r.borrow().iter().filter(|e| e.surface == surface).count())
    };
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    assert!(entries(2) > 0);

    // The side chat closes: its reset never paints again.
    view.update(cx, |harness, cx| {
        harness.second_pane = false;
        cx.notify();
    });
    for _ in 0..2 {
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
    }
    assert_eq!(entries(2), 0, "the closed pane's text is dropped");
    assert!(entries(1) > 0, "the open pane keeps its text");
}

#[gpui::test]
fn code_block_lines_participate_in_text_selection(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (_, cx) = cx.add_window_view(|_, _| CodeSelectionHarness::default());
    cx.simulate_resize(size(px(640.0), px(240.0)));
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });

    let before_key = "code-selection-test:0";
    let first_key = "code-selection-test-code1-line0";
    let blank_key = "code-selection-test-code1-line1";
    let second_key = "code-selection-test-code1-line2";
    let after_key = "code-selection-test:2";
    let before_bounds = selection_test_bounds(before_key);
    let first_bounds = selection_test_bounds(first_key);
    selection_test_bounds(blank_key);
    let second_bounds = selection_test_bounds(second_key);
    let after_bounds = selection_test_bounds(after_key);

    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: first_bounds.origin + point(px(5.0), px(9.0)),
        click_count: 2,
        ..Default::default()
    });
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("selectable")
    );
    super::super::selection::end_active_drag();
    super::super::selection::clear_if_owner(first_key);

    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position: first_bounds.origin + point(px(1.0), px(9.0)),
        click_count: 1,
        ..Default::default()
    });
    cx.simulate_event(gpui::MouseMoveEvent {
        position: point(second_bounds.right(), second_bounds.top() + px(9.0)),
        pressed_button: Some(gpui::MouseButton::Left),
        ..Default::default()
    });
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("selectable\n\nsecond")
    );
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Left,
        position: point(second_bounds.right(), second_bounds.top() + px(9.0)),
        ..Default::default()
    });
    super::super::selection::clear_if_owner(first_key);

    super::super::selection::begin(before_key, 0);
    assert!(update_drag_at(point(
        after_bounds.right(),
        after_bounds.top() + px(9.0)
    )));
    assert_eq!(
        super::super::selection::selected_text().as_deref(),
        Some("before\nselectable\n\nsecond\nafter")
    );
    super::super::selection::end_active_drag();
    super::super::selection::clear_if_owner(before_key);
    assert!(before_bounds.top() < first_bounds.top());
    assert!(second_bounds.bottom() < after_bounds.bottom());
}

#[gpui::test]
fn occluded_text_ignores_double_and_triple_clicks(cx: &mut TestAppContext) {
    let _selection = super::super::selection::test_state_lock();
    cx.update(|cx| cx.set_global(Theme::dark()));
    let (view, cx) = cx.add_window_view(|_, _| CodeSelectionHarness { occluded: true });
    cx.simulate_resize(size(px(640.0), px(240.0)));
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    let key = "code-selection-test-code1-line0";
    let position = selection_test_bounds(key).origin + point(px(5.0), px(9.0));
    for click_count in [2, 3] {
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position,
            click_count,
            ..Default::default()
        });
        let dragging = super::super::selection::is_dragging();
        let selected = super::super::selection::selected_text();
        cx.simulate_event(gpui::MouseUpEvent {
            button: gpui::MouseButton::Left,
            position,
            ..Default::default()
        });
        super::super::selection::clear_if_owner(key);
        assert_eq!((dragging, selected), (false, None));
    }
    view.update(cx, |view, cx| {
        view.occluded = false;
        cx.notify();
    });
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Left,
        position,
        click_count: 2,
        ..Default::default()
    });
    let selected = super::super::selection::selected_text();
    super::super::selection::clear_if_owner(key);
    assert_eq!(selected.as_deref(), Some("selectable"));
}
