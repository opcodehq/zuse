use super::*;
use gpui::{MouseClickEvent, MouseDownEvent, MouseUpEvent, TestAppContext, point};
#[test]
fn selection_drags_and_secondary_clicks_do_not_navigate() {
    let mut event = MouseClickEvent {
        down: MouseDownEvent {
            button: MouseButton::Left,
            click_count: 1,
            ..Default::default()
        },
        up: MouseUpEvent::default(),
    };
    assert!(click_is_activation(&ClickEvent::Mouse(event.clone())));
    event.up.position = point(px(20.), px(0.));
    assert!(!click_is_activation(&ClickEvent::Mouse(event.clone())));
    event.up.position = event.down.position;
    event.down.button = MouseButton::Right;
    assert!(!click_is_activation(&ClickEvent::Mouse(event.clone())));
    event.down.button = MouseButton::Left;
    event.down.click_count = 2;
    assert!(!click_is_activation(&ClickEvent::Mouse(event)));
}
#[gpui::test]
fn all_actions_keep_the_destination(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| gpui::Empty);
    window
        .update(cx, |_, window, cx| {
            let target = LinkTarget::new(
                "A misleading label",
                "https://example.com/full?query=yes#fragment",
            );
            activate_link(target.clone(), LinkAction::Copy, None, window, cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(target.original.as_str())
            );
            for action in [LinkAction::Internal, LinkAction::External] {
                let seen = Rc::new(RefCell::new(None));
                let captured = seen.clone();
                let ui = LinkUi {
                    source_session: Some("parent".into()),
                    source_local: false,
                    file_roots: None,
                    handler: Rc::new(move |a, _, _| {
                        *captured.borrow_mut() =
                            Some((a.target.clone(), a.action, a.source_session.clone()));
                        LinkOutcome::Rejected
                    }),
                };
                activate_link(target.clone(), action, Some(&ui), window, cx);
                assert_eq!(
                    *seen.borrow(),
                    Some((target.clone(), action, Some("parent".into())))
                );
            }
        })
        .unwrap();
}
