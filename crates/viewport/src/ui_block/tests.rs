use super::*;
use gpui::{TestAppContext, VisualTestContext};
use std::prelude::v1::test;

fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear());
    cx.run_until_parked();
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    source: &str,
    dark: bool,
) -> (Entity<UiBlockView>, &'a mut VisualTestContext) {
    cx.add_window_view(|_, cx| UiBlockView::new(source, UiBlockPalette::reference(dark), true, cx))
}

#[gpui::test]
fn every_demo_renders_at_wide_and_narrow_widths(cx: &mut TestAppContext) {
    cx.update(crate::input::bind_keys);
    for (title, source) in demo_specs() {
        for (dark, width) in [(true, 960.0), (false, 320.0)] {
            let (view, cx) = open(cx, source, dark);
            cx.simulate_resize(size(px(width), px(2400.)));
            frame(cx);
            // Second frame: width probes have reported, breakpoints apply.
            frame(cx);
            view.update(cx, |view, cx| {
                assert_eq!(view.is_valid(), !title.contains("invalid"), "{title}");
                // Responsive roots (Table, Flow, Grid) measured their width:
                // the window minus the block's 12px gutters.
                if let Some(measured) = view.widths.get("r") {
                    assert_eq!(measured.get(), width - 24.0, "{title}");
                }
                // Exercise hover, the open select menu and a second tab.
                view.line_hover.insert(
                    "r".into(),
                    LineHover {
                        index: 1,
                        cursor_x: 120.0,
                    },
                );
                view.line_hover.insert(
                    "r.3".into(),
                    LineHover {
                        index: 0,
                        cursor_x: 4.0,
                    },
                );
                view.open_select = Some("r.2".into());
                view.tabs.insert("r".into(), 1);
                cx.notify();
            });
            frame(cx);
            view.update(cx, |view, cx| {
                view.set_palette(UiBlockPalette::reference(!dark), cx);
                view.set_interactive(false, cx);
            });
            frame(cx);
        }
    }
}

#[gpui::test]
fn form_submission_emits_the_answer_and_locks(cx: &mut TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (_, source) = demo_specs()
        .iter()
        .find(|(title, _)| title.contains("every field"))
        .unwrap();
    let (view, cx) = open(cx, source, true);
    frame(cx);
    let sent = Rc::new(RefCell::new(Vec::new()));
    let events = sent.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &UiBlockEvent, _| {
            events.borrow_mut().push(event.clone());
        })
        .detach();
    });
    view.update(cx, |view, cx| {
        let root = view.root.clone().unwrap();
        let path = "r".to_string();
        view.set_field_value(&path, "terms", FieldValue::Bool(false));
        view.submit_form(&root, &path, cx);
        assert!(view.disabled());
        // A second submit while sending is ignored.
        view.submit_form(&root, &path, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        *sent.borrow(),
        vec![UiBlockEvent::Send {
            text: "Workspace request\n- Workspace name: —\n- What will you use it for? —\n- Region: eu-west\n- Plan: Team\n- I accept the usage policy: No\n- Monthly budget (USD): 120".into()
        }]
    );
    view.update(cx, |view, cx| {
        view.mark_send_failed(cx);
        assert!(!view.disabled());
        view.status = Status::Sending;
        view.mark_sent(cx);
        assert!(view.disabled());
        view.mark_send_failed(cx);
        assert!(view.disabled(), "a sent block stays locked");
    });
    frame(cx);
}

#[gpui::test]
fn non_interactive_blocks_never_send(cx: &mut TestAppContext) {
    cx.update(crate::input::bind_keys);
    let (view, cx) = cx.add_window_view(|_, cx| {
        UiBlockView::new(
            r#"root = FollowUps([{label: "Retry", prompt: "Retry the build"}])"#,
            UiBlockPalette::reference(false),
            false,
            cx,
        )
    });
    frame(cx);
    view.update(cx, |view, cx| {
        view.send("Retry the build".into(), cx);
        assert_eq!(view.status, Status::Idle);
        view.set_interactive(true, cx);
        view.send("Retry the build".into(), cx);
        assert_eq!(view.status, Status::Sending);
    });
    frame(cx);
}
