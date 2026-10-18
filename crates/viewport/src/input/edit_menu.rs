//! Shared text context menu for the browser and native form adapters.
use crate::palette::rgb;
use gpui::{prelude::*, *};
use gpui_base::input::{Copy, Cut, InputContextMenuCapabilities, NativeMenu, Paste, SelectAll};
use std::rc::Rc;

actions!(text_edit_menu, [Previous, Next, First, Last, Choose, Close]);

struct MenuBindings;
impl Global for MenuBindings {}

pub(crate) fn bind_keys(cx: &mut App) {
    if cx.has_global::<MenuBindings>() {
        return;
    }
    cx.set_global(MenuBindings);
    cx.bind_keys([
        KeyBinding::new("up", Previous, Some("TextEditMenu")),
        KeyBinding::new("down", Next, Some("TextEditMenu")),
        KeyBinding::new("home", First, Some("TextEditMenu")),
        KeyBinding::new("end", Last, Some("TextEditMenu")),
        KeyBinding::new("enter", Choose, Some("TextEditMenu")),
        KeyBinding::new("space", Choose, Some("TextEditMenu")),
        KeyBinding::new("escape", Close, Some("TextEditMenu")),
        KeyBinding::new("tab", Close, Some("TextEditMenu")),
        KeyBinding::new("shift-tab", Close, Some("TextEditMenu")),
        KeyBinding::new("ctrl-v", Paste, Some("TextEditMenu")),
        KeyBinding::new("cmd-v", Paste, Some("TextEditMenu")),
        KeyBinding::new("ctrl-c", Copy, Some("TextEditMenu")),
        KeyBinding::new("cmd-c", Copy, Some("TextEditMenu")),
        KeyBinding::new("ctrl-x", Cut, Some("TextEditMenu")),
        KeyBinding::new("cmd-x", Cut, Some("TextEditMenu")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextEditMenu")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextEditMenu")),
    ]);
}

pub(crate) struct EditMenu {
    at: Point<Pixels>,
    original: FocusHandle,
    focus: FocusHandle,
    enabled: [bool; 4],
    selected: usize,
}

pub(crate) trait Host: Sized + 'static {
    fn edit_menu(&mut self) -> &mut Option<EditMenu>;
}

type MenuHandler =
    Rc<dyn Fn(NativeMenu, InputContextMenuCapabilities, Point<Pixels>, &mut Window, &mut App)>;

pub(crate) fn handler<T: Host>(owner: WeakEntity<T>, original: FocusHandle) -> MenuHandler {
    Rc::new(move |_, capabilities, at, window, cx| {
        let _ = owner.update(cx, |this, cx| {
            // Native callers can embed a shared form without initializing the
            // browser workspace. Install these bindings once on either path.
            bind_keys(cx);
            let enabled = [
                capabilities.is_editable() && capabilities.has_selection(),
                capabilities.has_selection(),
                capabilities.is_editable(),
                true,
            ];
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            *this.edit_menu() = Some(EditMenu {
                at,
                original: original.clone(),
                focus,
                enabled,
                selected: enabled.iter().position(|enabled| *enabled).unwrap_or(3),
            });
            cx.notify();
        });
    })
}

fn close<T: Host>(this: &mut T, window: &mut Window, cx: &mut Context<T>) {
    if let Some(menu) = this.edit_menu().take() {
        window.focus(&menu.original, cx);
        cx.notify();
    }
}

fn choose<T: Host>(this: &mut T, index: usize, window: &mut Window, cx: &mut Context<T>) {
    let Some(menu) = this.edit_menu().take() else {
        return;
    };
    if !menu.enabled[index] {
        *this.edit_menu() = Some(menu);
        return;
    }
    window.focus(&menu.original, cx);
    let name = [
        "input::Cut",
        "input::Copy",
        "input::Paste",
        "input::SelectAll",
    ][index];
    if let Ok(action) = cx.build_action(name, None) {
        window.dispatch_action(action, cx);
    }
    cx.notify();
}

fn navigate<T: Host>(this: &mut T, delta: isize, cx: &mut Context<T>) {
    if let Some(menu) = this.edit_menu().as_mut() {
        for _ in 0..4 {
            menu.selected = (menu.selected as isize + delta).rem_euclid(4) as usize;
            if menu.enabled[menu.selected] {
                break;
            }
        }
        cx.notify();
    }
}

pub(crate) fn render<T: Host>(menu: &EditMenu, cx: &mut Context<T>) -> AnyElement {
    let labels = ["Cut", "Copy", "Paste", "Select all"];
    let card = div()
        .id("text-edit-menu")
        .role(Role::Menu)
        .key_context("TextEditMenu")
        .track_focus(&menu.focus)
        .w(px(180.))
        .p(px(4.))
        .rounded(px(8.))
        .bg(rgb(crate::shell::SURFACE))
        .border_1()
        .border_color(rgb(crate::shell::BORDER))
        .text_color(rgb(crate::shell::TEXT))
        .text_size(px(12.))
        .on_mouse_down_out(cx.listener(|this, _, window, cx| close(this, window, cx)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_action(cx.listener(|this, _: &Previous, _, cx| navigate(this, -1, cx)))
        .on_action(cx.listener(|this, _: &Next, _, cx| navigate(this, 1, cx)))
        .on_action(cx.listener(|this, _: &First, _, cx| {
            if let Some(menu) = this.edit_menu().as_mut() {
                menu.selected = 3;
            }
            navigate(this, 1, cx);
        }))
        .on_action(cx.listener(|this, _: &Last, _, cx| {
            if let Some(menu) = this.edit_menu().as_mut() {
                menu.selected = 0;
            }
            navigate(this, -1, cx);
        }))
        .on_action(cx.listener(|this, _: &Close, window, cx| close(this, window, cx)))
        .on_action(cx.listener(|this, _: &Choose, window, cx| {
            let selected = this.edit_menu().as_ref().map(|menu| menu.selected);
            if let Some(selected) = selected {
                choose(this, selected, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &Cut, window, cx| choose(this, 0, window, cx)))
        .on_action(cx.listener(|this, _: &Copy, window, cx| choose(this, 1, window, cx)))
        .on_action(cx.listener(|this, _: &Paste, window, cx| choose(this, 2, window, cx)))
        .on_action(cx.listener(|this, _: &SelectAll, window, cx| choose(this, 3, window, cx)))
        .on_key_down(|_, _, cx| cx.stop_propagation())
        .children(labels.into_iter().enumerate().map(|(index, label)| {
            let enabled = menu.enabled[index];
            div()
                .id(("text-edit-action", index))
                .role(Role::MenuItem)
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .when(enabled, |row| {
                    row.cursor_pointer()
                        .hover(|row| row.bg(rgb(crate::shell::RAISED)))
                })
                .when(!enabled, |row| row.opacity(0.4))
                .when(index == menu.selected, |row| {
                    row.bg(rgb(crate::shell::RAISED))
                })
                .child(label)
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if let Some(menu) = this.edit_menu().as_mut() {
                        if enabled && menu.selected != index {
                            menu.selected = index;
                            cx.notify();
                        }
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if enabled {
                        choose(this, index, window, cx);
                    }
                    cx.stop_propagation();
                }))
        }));
    deferred(
        anchored()
            .position(menu.at)
            .snap_to_window_with_margin(px(8.))
            .child(card),
    )
    .priority(100)
    .into_any_element()
}
