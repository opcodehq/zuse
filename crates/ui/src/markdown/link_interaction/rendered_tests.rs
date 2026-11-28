use super::*;
use gpui::{Context, Render};
#[cfg(windows)]
use gpui_platform::application as test_application;
#[cfg(target_os = "linux")]
use gpui_platform::headless as test_application;
thread_local! {
    pub(super) static TOOLTIP_BOUNDS: RefCell<Option<Rc<Cell<Option<Bounds<Pixels>>>>>> = RefCell::default();
}
fn draw_has_tooltip(window: &mut Window, cx: &mut App) -> bool {
    TOOLTIP_BOUNDS.with(|bounds| {
        if let Some(bounds) = bounds.borrow().as_ref() {
            bounds.set(None);
        }
    });
    window.refresh();
    let _ = window.draw(cx);
    TOOLTIP_BOUNDS.with(|bounds| bounds.borrow().as_ref().is_some_and(|b| b.get().is_some()))
}
fn key(window: &mut Window, key: &str, cx: &mut App) {
    window.dispatch_event(
        gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
            is_held: false,
            prefer_character_input: false,
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse(key).unwrap(),
        }),
        cx,
    );
    window.refresh();
    let _ = window.draw(cx);
}
struct Fixture {
    markdown: String,
    width: f32,
    activated: Rc<RefCell<Vec<LinkActivation>>>,
    file_roots: Option<Rc<Vec<crate::workspace_links::FileLinkRoot>>>,
    source_local: bool,
}
impl Render for Fixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut opts = super::super::render::RenderOptions::settled("link-fixture".into());
        let activated = self.activated.clone();
        opts.link = Some(LinkUi {
            source_session: Some("session".into()),
            source_local: self.source_local,
            file_roots: self.file_roots.clone(),
            handler: Rc::new(move |a, _, _| {
                activated.borrow_mut().push(a.clone());
                LinkOutcome::Rejected
            }),
        });
        let tree = super::super::parser::parse_full(&self.markdown);
        div()
            .w(px(self.width))
            .child(super::super::render::selection_frame_reset())
            .child(super::super::render::render_tree(
                &tree,
                &opts,
                &Theme::of(cx).clone(),
                window,
                &|_, _| None,
            ))
    }
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires a native desktop; run with --ignored --test-threads=1"
)]
fn escape_dismisses_focused_destination() {
    test_application().run(|cx| {
        cx.set_global(Theme::dark());
        let window = cx
            .open_window(Default::default(), |_, cx| {
                cx.new(|_| Fixture {
                    markdown: "[Docs](https://example.com/docs)".into(),
                    width: 320.,
                    activated: Rc::default(),
                    source_local: false,
                    file_roots: None,
                })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            draw_has_tooltip(window, cx);
            window.focus_next(cx);
            assert!(
                draw_has_tooltip(window, cx),
                "focused link shows destination"
            );
            key(window, "escape", cx);
            assert!(
                !draw_has_tooltip(window, cx),
                "Escape must dismiss destination"
            );
        })
        .unwrap();
        cx.spawn(async move |cx| {
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
struct OutsideClickFixture {
    link: gpui::Entity<Fixture>,
    clicks: Rc<Cell<usize>>,
}
impl Render for OutsideClickFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div().size_full().child(self.link.clone()).child(
            div()
                .id("outside-button")
                .absolute()
                .left(px(400.))
                .top(px(100.))
                .size(px(50.))
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
        )
    }
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires a native desktop; run with --ignored --test-threads=1"
)]
fn focused_destination_does_not_block_other_controls() {
    test_application().run(|cx| {
        cx.set_global(Theme::dark());
        let clicks = Rc::new(Cell::new(0));
        let window = cx
            .open_window(Default::default(), |_, cx| {
                let link = cx.new(|_| Fixture {
                    markdown: "[Docs](https://example.com/docs)".into(),
                    width: 320.,
                    activated: Rc::default(),
                    source_local: false,
                    file_roots: None,
                });
                cx.new(|_| OutsideClickFixture {
                    link,
                    clicks: clicks.clone(),
                })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            draw_has_tooltip(window, cx);
            let (_, layout, _) = super::super::render::selection_test_snapshot("link-fixture:0");
            let link_position = range_rects(&layout, &(0..4), 0., 0.)[0].center();
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: link_position,
                    ..Default::default()
                }),
                cx,
            );
            draw_has_tooltip(window, cx);
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: MouseButton::Left,
                    position: link_position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            draw_has_tooltip(window, cx);
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: MouseButton::Left,
                    position: link_position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(draw_has_tooltip(window, cx));
            let position = gpui::point(px(425.), px(125.));
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            draw_has_tooltip(window, cx);
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert_eq!(
                clicks.get(),
                1,
                "destination must not swallow other controls' clicks"
            );
            assert!(
                !draw_has_tooltip(window, cx),
                "outside click dismisses destination"
            );
        })
        .unwrap();
        cx.spawn(async move |cx| {
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires a native desktop; run with --ignored --test-threads=1"
)]
fn context_menu_cancels_visible_and_pending_hover_tooltips() {
    let dir = tempfile::tempdir().unwrap();
    test_application().run(move |cx| {
        cx.set_global(Theme::dark());
        crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        // Keep the headless event loop alive between scenario windows.
        cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::Empty))
            .unwrap();
        cx.spawn(async move |cx| {
            let _settings_dir = dir;
            for visible in [true, false] {
                for keyboard in [false, true] {
                    let activated = Rc::new(RefCell::new(Vec::new()));
                    let window = cx.update(|cx| {
                        cx.open_window(Default::default(), |_, cx| {
                            cx.new(|_| Fixture {
                                markdown: "[Docs](https://example.com/docs)".into(),
                                width: 320.,
                                activated: activated.clone(),
                                source_local: false,
                                file_roots: None,
                            })
                        })
                        .unwrap()
                    });
                    let position = cx
                        .update_window(window.into(), |_, window, cx| {
                            draw_has_tooltip(window, cx);
                            let (_, layout, _) =
                                super::super::render::selection_test_snapshot("link-fixture:0");
                            let position = range_rects(&layout, &(0..4), 0., 0.)[0].center();
                            window.dispatch_event(
                                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                    position,
                                    ..Default::default()
                                }),
                                cx,
                            );
                            draw_has_tooltip(window, cx);
                            position
                        })
                        .unwrap();
                    if visible {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(650))
                            .await;
                        cx.update_window(window.into(), |_, window, cx| {
                            assert!(
                                draw_has_tooltip(window, cx),
                                "hover should show the destination"
                            );
                        })
                        .unwrap();
                    }
                    cx.update_window(window.into(), |_, window, cx| {
                        if keyboard {
                            window.focus_next(cx);
                            draw_has_tooltip(window, cx);
                            key(window, "shift-f10", cx);
                        } else {
                            window.dispatch_event(
                                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                    button: MouseButton::Right,
                                    position,
                                    click_count: 1,
                                    ..Default::default()
                                }),
                                cx,
                            );
                            window.dispatch_event(
                                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                    button: MouseButton::Right,
                                    position,
                                    click_count: 1,
                                    ..Default::default()
                                }),
                                cx,
                            );
                        }
                        assert!(
                            !draw_has_tooltip(window, cx),
                            "menu must hide an already visible tooltip"
                        );
                    })
                    .unwrap();
                    // Leave the pointer over the link beyond the show delay.
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(650))
                        .await;
                    cx.update_window(window.into(), |_, window, cx| {
                        assert!(
                            !draw_has_tooltip(window, cx),
                            "pending hover must not appear over the menu"
                        );
                        let before = crate::settings::current(cx).open_web_links_in_zuse;
                        key(window, "down", cx);
                        key(window, "down", cx);
                        key(window, "down", cx);
                        key(window, "enter", cx);
                        assert_ne!(
                            crate::settings::current(cx).open_web_links_in_zuse,
                            before,
                            "the fourth menu row toggles the default destination"
                        );
                        assert!(activated.borrow().is_empty());
                        window.remove_window();
                    })
                    .unwrap();
                }
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires a native desktop; run with --ignored --test-threads=1"
)]
fn keyboard_visits_each_range_and_opens_the_link_menu() {
    test_application().run(|cx| {
        cx.set_global(Theme::dark());
        let activated = Rc::new(RefCell::new(Vec::new()));
        let log = activated.clone();
        let window = cx
            .open_window(Default::default(), |_, cx| {
                cx.new(|_| Fixture {
                    activated,
                    width: 320.,
                    source_local: false,
                    file_roots: None,
                    markdown:
                        "[first](https://example.com/one) and [second](https://example.org/two)"
                            .into(),
                })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            window.focus_next(cx);
            window.refresh();
            let _ = window.draw(cx);
            for key in ["enter", "tab", "enter", "shift-f10"] {
                window.dispatch_event(
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke: gpui::Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                        keystroke: gpui::Keystroke::parse(key).unwrap(),
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
            }
            assert_eq!(log.borrow().len(), 2);
            assert_eq!(log.borrow()[0].target.original, "https://example.com/one");
            assert_eq!(log.borrow()[1].target.original, "https://example.org/two");
            // Menu starts on Open in Zuse; choose the external action.
            for key in ["down", "enter"] {
                window.dispatch_event(
                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                        keystroke: gpui::Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                        keystroke: gpui::Keystroke::parse(key).unwrap(),
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
            }
            assert_eq!(log.borrow().len(), 3);
            assert_eq!(log.borrow()[2].action, LinkAction::External);
            assert_eq!(log.borrow()[2].target.original, "https://example.org/two");
        })
        .unwrap();
        cx.spawn(async move |cx| {
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
#[test]
#[cfg_attr(
    windows,
    ignore = "requires a native desktop; run with --ignored --test-threads=1"
)]
fn rendered_truncation_resizes_and_selects_the_original_url() {
    test_application().run(|cx| {
        cx.set_global(Theme::dark());
        let url = format!("https://example.com/{}", "long-segment-🙂/".repeat(20));
        let markdown = format!("[{url}]({url})");
        let window = cx
            .open_window(Default::default(), |_, cx| {
                cx.new(|_| Fixture {
                    activated: Rc::default(),
                    width: 180.,
                    markdown,
                    source_local: false,
                    file_roots: None,
                })
            })
            .unwrap();
        let view = window.entity(cx).unwrap();
        for width in [180., 420., 100.] {
            view.update(cx, |view, cx| {
                view.width = width;
                cx.notify();
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                let (original, layout, offsets) =
                    super::super::render::selection_test_snapshot("link-fixture:0");
                assert_eq!(original.as_ref(), url);
                let offsets = offsets.unwrap();
                assert_eq!(offsets.omissions.len(), 1);
                assert!(layout.bounds().size.width <= px(width));
                assert!(layout.bounds().size.height <= px(23.));
                let shown_end = offsets.displayed(url.len());
                let start = layout.position_for_index(0).unwrap() + gpui::point(px(0.1), px(8.));
                let end =
                    layout.position_for_index(shown_end).unwrap() + gpui::point(px(0.1), px(8.));
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: MouseButton::Left,
                        position: start,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.refresh();
                let _ = window.draw(cx);
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: end,
                        pressed_button: Some(MouseButton::Left),
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button: MouseButton::Left,
                        position: end,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                assert_eq!(
                    super::super::selection::selected_text().as_deref(),
                    Some(url.as_str())
                );
                assert!(view.read(cx).activated.borrow().is_empty());
                super::super::selection::clear_if_owner("link-fixture:0");
            })
            .unwrap();
        }
        view.update(cx, |view, cx| {
            view.width = 220.;
            view.markdown = format!("| [{url}]({url}) | Notes |\n| --- | --- |\n| short | cell |");
            cx.notify();
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            let cell = super::super::render::table_cell_ix(0, 0, 0);
            let (original, layout, offsets) =
                super::super::render::selection_test_snapshot(&format!("link-fixture:{cell}"));
            assert_eq!(original.as_ref(), url);
            assert_eq!(offsets.unwrap().omissions.len(), 1);
            assert!(layout.bounds().size.width < px(220.));
            assert!(layout.bounds().size.height <= px(23.));
        })
        .unwrap();
        cx.spawn(async move |cx| {
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

/// A resolved file link's context menu swaps the web actions for file
/// rows — the system-level ones only when the owning root is on this
/// device — which act on the resolved absolute path, and "Open in
/// Zuse" routes the file to the viewer.
#[cfg(unix)]
#[gpui::test]
fn file_link_menu_offers_local_rows_and_acts_on_the_absolute_path(cx: &mut gpui::TestAppContext) {
    assert_local_file_link_menu("src/lib.rs", cx);
}

#[cfg(unix)]
#[gpui::test]
fn markdown_absolute_path_has_the_same_file_menu(cx: &mut gpui::TestAppContext) {
    assert_local_file_link_menu("/repo dir/src/lib.rs", cx);
}

#[cfg(unix)]
#[gpui::test]
fn markdown_file_url_has_the_same_file_menu(cx: &mut gpui::TestAppContext) {
    assert_local_file_link_menu("file:///repo dir/src/lib.rs", cx);
}

#[cfg(unix)]
fn assert_local_file_link_menu(destination: &str, cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
    });
    let roots = Rc::new(vec![crate::workspace_links::FileLinkRoot {
        chat: Some("chat".into()),
        root: "/repo dir".into(),
        local: true,
    }]);
    let activated: Rc<RefCell<Vec<LinkActivation>>> = Rc::default();
    let (_view, cx) = cx.add_window_view(|_, _| Fixture {
        markdown: format!("see [lib]({destination}) here").into(),
        width: 320.,
        activated: activated.clone(),
        source_local: false,
        file_roots: Some(roots),
    });
    let position = cx.update(|_, _| {
        let (_, layout, _) = super::super::render::selection_test_snapshot("link-fixture:0");
        layout.position_for_index(4).unwrap() + gpui::point(px(2.), px(8.))
    });
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    for selector in [
        "link-menu-open-zuse",
        "link-menu-open-default",
        "link-menu-show-in-folder",
        "link-menu-copy-path",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} should be mounted for a local file link"
        );
    }
    for selector in [
        "link-menu-open-external",
        "link-menu-copy-address",
        "link-menu-default-destination",
    ] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "{selector} is a web-link row"
        );
    }

    // "Open with default app" sends the resolved path to the platform
    // opener as a file:// URL — spaces stay percent-encoded.
    let row = cx.debug_bounds("link-menu-open-default").unwrap().center();
    cx.simulate_click(row, gpui::Modifiers::default());
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("file:///repo%20dir/src/lib.rs")
    );
    assert!(cx.debug_bounds("link-menu-open-zuse").is_none());

    // "Copy file path" copies the resolved absolute path, decoded.
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    let row = cx.debug_bounds("link-menu-copy-path").unwrap().center();
    cx.simulate_click(row, gpui::Modifiers::default());
    let copied = cx.cx.update(|cx| cx.read_from_clipboard());
    assert_eq!(
        copied.and_then(|item| item.text()).as_deref(),
        Some("/repo dir/src/lib.rs")
    );

    // "Open in Zuse" is live for a file link and hands it to the
    // owning surface as an internal open.
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    let row = cx.debug_bounds("link-menu-open-zuse").unwrap().center();
    cx.simulate_click(row, gpui::Modifiers::default());
    let activated = activated.borrow();
    let last = activated.last().expect("open in zuse activates the link");
    assert_eq!(last.action, LinkAction::Internal);
    assert_eq!(last.target.original, destination);
}

/// An absolute destination no root owns is still a file: its menu keeps
/// "Open in Zuse" and, when the linking chat is on this device, the
/// system-level rows that act on the absolute path.
#[cfg(unix)]
#[gpui::test]
fn outside_file_link_menu_keeps_open_in_zuse_and_local_rows(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
    });
    let roots = Rc::new(vec![crate::workspace_links::FileLinkRoot {
        chat: Some("chat".into()),
        root: "/repo".into(),
        local: true,
    }]);
    let activated: Rc<RefCell<Vec<LinkActivation>>> = Rc::default();
    let (_view, cx) = cx.add_window_view(|_, _| Fixture {
        markdown: "see [report](/elsewhere/team/INFORME.md) here".into(),
        width: 320.,
        activated: activated.clone(),
        source_local: true,
        file_roots: Some(roots),
    });
    let position = cx.update(|_, _| {
        let (_, layout, _) = super::super::render::selection_test_snapshot("link-fixture:0");
        layout.position_for_index(4).unwrap() + gpui::point(px(2.), px(8.))
    });
    cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
    cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
    for selector in [
        "link-menu-open-zuse",
        "link-menu-open-default",
        "link-menu-show-in-folder",
        "link-menu-copy-path",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "{selector} should be mounted for an outside link on this device"
        );
    }
    for selector in [
        "link-menu-open-external",
        "link-menu-copy-address",
        "link-menu-default-destination",
    ] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "{selector} is a web-link row"
        );
    }
    let row = cx.debug_bounds("link-menu-open-zuse").unwrap().center();
    cx.simulate_click(row, gpui::Modifiers::default());
    let activated = activated.borrow();
    let last = activated.last().expect("open in zuse activates the link");
    assert_eq!(last.action, LinkAction::Internal);
    assert_eq!(last.target.original, "/elsewhere/team/INFORME.md");
}

/// A root on another device resolves the link (the file opens through
/// that chat's context) but cannot offer local system actions; a web
/// link shows only the web rows.
#[gpui::test]
fn file_link_menu_hides_system_rows_for_remote_and_web_links(cx: &mut gpui::TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
    });
    for (markdown, roots, has_copy_path) in [
        (
            "see [lib](src/lib.rs) here",
            Some(Rc::new(vec![crate::workspace_links::FileLinkRoot {
                chat: Some("remote".into()),
                root: "/far/worktree".into(),
                local: false,
            }])),
            true,
        ),
        ("see [docs](https://example.com/) here", None, false),
    ] {
        let (_view, cx) = cx.add_window_view(|_, _| Fixture {
            markdown: markdown.into(),
            width: 320.,
            activated: Rc::default(),
            source_local: false,
            file_roots: roots.clone(),
        });
        let position = cx.update(|_, _| {
            let (_, layout, _) = super::super::render::selection_test_snapshot("link-fixture:0");
            layout.position_for_index(4).unwrap() + gpui::point(px(2.), px(8.))
        });
        cx.simulate_mouse_down(position, MouseButton::Right, gpui::Modifiers::default());
        cx.simulate_mouse_up(position, MouseButton::Right, gpui::Modifiers::default());
        assert!(cx.debug_bounds("link-menu-open-zuse").is_some());
        for selector in [
            "link-menu-open-external",
            "link-menu-copy-address",
            "link-menu-default-destination",
        ] {
            assert_eq!(
                cx.debug_bounds(selector).is_some(),
                !has_copy_path,
                "{selector} mounts only for web links"
            );
        }
        for selector in ["link-menu-open-default", "link-menu-show-in-folder"] {
            assert!(
                cx.debug_bounds(selector).is_none(),
                "{selector} needs a local file root"
            );
        }
        assert_eq!(
            cx.debug_bounds("link-menu-copy-path").is_some(),
            has_copy_path,
            "copy-path row matches file resolution"
        );
    }
}
