//! App selection belongs to the file toolbar that opened it. The selected
//! path is captured on open; the workspace titlebar keeps its own menu.
use super::{FilesEvent, FilesSurface, toolbar_button};
use crate::{
    icons, popover,
    shell::open_in::{self, OpenTarget},
    theme::Theme,
};
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, KeyDownEvent, MouseButton, SharedString, Task,
    Window, div, prelude::*, px,
};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct FileOpenIn {
    menu: popover::Popup<PathBuf>,
    targets: Vec<OpenTarget>,
    detected: bool,
    detection: Option<Task<()>>,
    focus: Option<FocusHandle>,
    active: usize,
}

impl FilesSurface {
    pub(super) fn close_file_open_in(&mut self, cx: &mut Context<Self>) {
        if self.preview.open_in.menu.begin_close() {
            popover::reap_popup(cx, |surface: &mut Self| &mut surface.preview.open_in.menu);
            cx.notify();
        }
    }

    fn choose_file_open_in(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(path) = self.preview.open_in.menu.as_open().cloned() else {
            return;
        };
        let Some(target) = self.preview.open_in.targets.get(index).cloned() else {
            return;
        };
        self.close_file_open_in(cx);
        cx.emit(FilesEvent::OpenIn { path, target });
    }

    fn copy_file_open_in_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.preview.open_in.menu.as_open().cloned() else {
            return;
        };
        self.close_file_open_in(cx);
        cx.write_to_clipboard(ClipboardItem::new_string(
            path.to_string_lossy().into_owned(),
        ));
    }

    fn open_file_picker(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.preview.open_in.active = 0;
        self.preview.open_in.menu.open(path);
        let focus = self
            .preview
            .open_in
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        focus.focus(window, cx);
        if !self.preview.open_in.detected && self.preview.open_in.detection.is_none() {
            self.preview.open_in.detection = Some(cx.spawn(async move |this, cx| {
                let targets = cx
                    .background_executor()
                    .spawn(async {
                        open_in::detect_targets(
                            &std::env::temp_dir().join("zuse-open-target-icons"),
                        )
                    })
                    .await;
                let _ = this.update(cx, |surface, cx| {
                    surface.preview.open_in.targets = targets;
                    surface.preview.open_in.detected = true;
                    surface.preview.open_in.detection = None;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn file_open_in_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke.key;
        let mods = event.keystroke.modifiers;
        if key == "escape" {
            self.close_file_open_in(cx);
            self.focus_editor(window, cx);
        } else if key == "c" && mods.shift && (mods.platform || mods.control) {
            self.copy_file_open_in_path(cx);
            self.focus_editor(window, cx);
        } else if matches!(key.as_str(), "up" | "down") {
            let count = self.preview.open_in.targets.len() + 1;
            self.preview.open_in.active = if key == "up" {
                (self.preview.open_in.active + count - 1) % count
            } else {
                (self.preview.open_in.active + 1) % count
            };
            cx.notify();
        } else if matches!(key.as_str(), "enter" | "return") {
            let index = self.preview.open_in.active;
            if index == self.preview.open_in.targets.len() {
                self.copy_file_open_in_path(cx);
            } else {
                self.choose_file_open_in(index, cx);
            }
            self.focus_editor(window, cx);
        } else if !mods.platform && !mods.control && !mods.alt {
            if let Ok(number) = key.parse::<usize>() {
                if number > 0 {
                    self.choose_file_open_in(number - 1, cx);
                }
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn render_file_open_in(
        &mut self,
        path: PathBuf,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.preview.open_in.menu.is_open();
        let mut trigger = toolbar_button("files-open-with-system", "Open in…")
            .debug_selector(|| "files-open-in-trigger".into())
            .relative()
            .w(px(78.0))
            .gap(px(5.0))
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .aria_expanded(open)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.preview.open_in.menu.note_trigger_press();
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if this.preview.open_in.menu.take_press_was_open() {
                    this.close_file_open_in(cx);
                } else {
                    this.open_file_picker(path.clone(), window, cx);
                }
            }))
            .child("Open in")
            .child(
                icons::icon(icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            );
        if self.preview.open_in.menu.get().is_some() {
            let theme = theme.for_popup();
            let focus = self
                .preview
                .open_in
                .focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let targets = self.preview.open_in.targets.clone();
            let active = self.preview.open_in.active;
            let loading = self.preview.open_in.detection.is_some();
            let menu = popover::popover_card(&theme)
                .id("files-open-in-card")
                .debug_selector(|| "files-open-in-card".into())
                .min_w(px(224.0))
                .flex()
                .flex_col()
                .track_focus(&focus)
                .role(gpui::Role::Menu)
                .aria_label("Open file in application")
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(|_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(Self::file_open_in_key))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_file_open_in(cx)))
                .when(loading, |el| {
                    el.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .text_color(theme.text_muted)
                            .child("Finding applications…"),
                    )
                })
                .children(targets.iter().enumerate().map(|(index, target)| {
                    popover::menu_row(
                        &theme,
                        active == index,
                        format!("file-open-in-{}", target.id),
                    )
                    .id(SharedString::from(format!("file-open-in-{}", target.id)))
                    .debug_selector(move || format!("file-open-in-target-{index}"))
                    .role(gpui::Role::MenuItem)
                    .aria_label(target.label)
                    .gap(px(12.0))
                    .child(open_in::target_icon(target, &theme))
                    .child(div().flex_1().child(target.label))
                    .child(popover::kbd_hint(&theme, &(index + 1).to_string()))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.choose_file_open_in(index, cx)),
                    )
                }))
                .child(popover::menu_separator())
                .child(
                    popover::menu_row(&theme, active == targets.len(), "file-open-in-copy")
                        .id("file-open-in-copy")
                        .role(gpui::Role::MenuItem)
                        .aria_label("Copy file path")
                        .child(
                            icons::icon(icons::COPY)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().child("Copy path"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.copy_file_open_in_path(cx);
                            this.focus_editor(window, cx);
                        })),
                );
            trigger = trigger.child(popover::anchored_menu_below_end(
                "files-open-in-menu",
                menu.into_any_element(),
                self.preview.open_in.menu.closing_since(),
            ));
        }
        trigger.into_any_element()
    }

    #[cfg(feature = "browser-fixture")]
    pub(crate) fn fixture_open_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview.open_in.targets = open_in::fixture_targets();
        self.preview.open_in.detected = true;
        if let Some(path) = self
            .preview
            .active
            .as_deref()
            .and_then(|path| self.system_open_path(path))
        {
            self.open_file_picker(path, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};

    fn setup(cx: &mut TestAppContext) -> (gpui::Entity<FilesSurface>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let (files, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| crate::files::test_support::state());
            let mut files = FilesSurface::new_editor(
                state,
                "chat".into(),
                "a.txt".into(),
                false,
                500,
                13.0,
                false,
                false,
                cx,
            );
            files.sync_target(cx);
            files.open_file("a.txt".into(), cx);
            files.preview.open_in.targets = open_in::fixture_targets();
            files.preview.open_in.detected = true;
            files
        });
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear();
        });
        (files, cx)
    }

    #[gpui::test]
    fn file_picker_is_anchored_below_the_file_toolbar_and_escape_closes_it(
        cx: &mut TestAppContext,
    ) {
        let (files, cx) = setup(cx);
        let trigger = cx
            .debug_bounds("files-open-in-trigger")
            .expect("file toolbar button");
        cx.simulate_click(trigger.center(), Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let menu = cx
            .debug_bounds("files-open-in-card")
            .expect("file picker dropdown");
        assert!(menu.top() >= trigger.bottom());
        assert!(menu.right() <= trigger.right() + px(1.0));
        files.read_with(cx, |files, _| assert!(files.preview.open_in.menu.is_open()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        files.read_with(cx, |files, _| {
            assert!(!files.preview.open_in.menu.is_open())
        });
    }

    #[gpui::test]
    fn file_picker_app_click_emits_file_action_without_reopening_the_trigger(
        cx: &mut TestAppContext,
    ) {
        let (files, cx) = setup(cx);
        let received = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&files, {
                let received = received.clone();
                move |_, event, _| {
                    if let FilesEvent::OpenIn { path, target } = event {
                        received.borrow_mut().push((path.clone(), target.id));
                    }
                }
            })
        });
        let trigger = cx.debug_bounds("files-open-in-trigger").unwrap();
        cx.simulate_click(trigger.center(), Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let app = cx
            .debug_bounds("file-open-in-target-1")
            .expect("Cursor app row");
        cx.simulate_click(app.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            &*received.borrow(),
            &[(PathBuf::from("/workspace/a.txt"), "cursor")]
        );
        files.read_with(cx, |files, _| {
            assert!(!files.preview.open_in.menu.is_open())
        });
    }

    #[gpui::test]
    fn file_picker_opens_the_captured_file_once_even_if_the_workspace_changes(
        cx: &mut TestAppContext,
    ) {
        let (files, cx) = setup(cx);
        let received = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&files, {
                let received = received.clone();
                move |_, event, _| {
                    if let FilesEvent::OpenIn { path, target } = event {
                        received.borrow_mut().push((path.clone(), target.id));
                    }
                }
            })
        });
        files.update_in(cx, |files, window, cx| {
            files.open_file_picker(PathBuf::from("/workspace/a.txt"), window, cx);
            files.state.update(cx, |state, _| {
                state.chats[0].cwd = Some("/different/workspace".into())
            });
            files.choose_file_open_in(1, cx);
            files.choose_file_open_in(1, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            &*received.borrow(),
            &[(PathBuf::from("/workspace/a.txt"), "cursor")]
        );
    }
}
