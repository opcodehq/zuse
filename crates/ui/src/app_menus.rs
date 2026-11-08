//! Native menu bar + app-level window actions (macOS-first).
//!
//! zuse never called `cx.set_menus`, so on macOS `NSApp.mainMenu` stayed nil:
//! no app menu, no ⌘Q quit, and nothing for the auto-hidden system menu bar to
//! reveal on hover (gpui only calls `setMainMenu_` from `set_menus` —
//! gpui_macos/src/platform.rs `fn set_menus`). Structure ported from zed's
//! `crates/zed/src/zed/app_menus.rs` and the gpui `set_menus.rs` example at the
//! pinned rev (f14fea9bf3c9).
//!
//! Wiring: [`init`] registers the global action handlers (run once at boot),
//! [`bind_keys`] installs the fixed application shortcuts (re-run by
//! `shell::apply_keymap`, which clears every binding first), and
//! [`app_menus`] builds the menu bar handed to `cx.set_menus` in `run_app`.

use std::time::{Duration, Instant};

use gpui::{App, Global, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, Window, actions};

use crate::appearance::{self, AppearanceMode};
use crate::composer;
use crate::settings::QuitGesture;
use crate::shell;

actions!(
    zuse,
    [
        About,
        CheckForUpdates,
        Quit,
        QuitShortcut,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        CloseWindow,
        AppearanceSystem,
        AppearanceLight,
        AppearanceDark,
    ]
);

/// Register the global handlers backing the menu bar and its shortcuts. Call
/// once at boot, before `cx.set_menus`.
pub fn init(cx: &mut App) {
    #[cfg(all(target_os = "macos", not(test)))]
    native_quit::init(cx);
    cx.on_action(quit);
    cx.on_action(quit_shortcut);
    #[cfg(target_os = "macos")]
    cx.on_action(|_: &About, _| about_panel::show());
    // Global, not per-window: it must work with every window closed, and the
    // update lifecycle is app-wide (see `app_update`).
    cx.on_action(|_: &CheckForUpdates, cx| crate::app_update::check_for_updates(cx));
    // Application-menu verbs — gpui wraps NSApp `hide` / `hideOtherApplications`
    // / `unhideAllApplications` (zed registers the same trio in
    // crates/zed/src/zed.rs `init`).
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    // Window verbs route to the active window. zuse is single-window, so a
    // global handler suffices where zed registers these per-workspace
    // (crates/zed/src/zed.rs `register_action(Minimize/Zoom)`).
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window| window.zoom_window()));
    cx.on_action(close_window);
    // Appearance. Each verb persists and repaints every window; see
    // `appearance::set_mode`.
    cx.on_action(|_: &AppearanceSystem, cx| appearance::set_mode(AppearanceMode::System, cx));
    cx.on_action(|_: &AppearanceLight, cx| appearance::set_mode(AppearanceMode::Light, cx));
    cx.on_action(|_: &AppearanceDark, cx| appearance::set_mode(AppearanceMode::Dark, cx));
}

fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window)) {
    if let Some(window) = cx.active_window() {
        window.update(cx, |_, window, _| f(window)).ok();
    }
}

/// All app-owned quit paths must flush file buffers before GPUI destroys windows.
fn quit(_: &Quit, cx: &mut App) {
    request_quit(cx);
}

/// How long after the first ⌘Q a second one still quits.
const DOUBLE_PRESS_WINDOW: Duration = Duration::from_millis(1200);
const HOLD_TO_QUIT: Duration = Duration::from_millis(800);
/// Longer than the usual delay before a held key starts repeating; a gap
/// beyond it means the key was let go.
const HOLD_RELEASE_GAP: Duration = Duration::from_millis(600);

#[derive(Debug, PartialEq, Eq)]
enum QuitPress {
    Quit,
    Hint,
    Wait,
}

/// ⌘Q presses so far. A held key arrives as repeated presses.
#[derive(Default)]
struct QuitPresses {
    started: Option<Instant>,
    last: Option<Instant>,
}

impl Global for QuitPresses {}

impl QuitPresses {
    fn press(&mut self, gesture: QuitGesture, now: Instant) -> QuitPress {
        let since_last = self.last.map(|last| now.saturating_duration_since(last));
        self.last = Some(now);
        match gesture {
            QuitGesture::Press => QuitPress::Quit,
            QuitGesture::DoublePress => {
                if since_last.is_some_and(|gap| gap <= DOUBLE_PRESS_WINDOW) {
                    self.last = None;
                    QuitPress::Quit
                } else {
                    QuitPress::Hint
                }
            }
            QuitGesture::Hold => {
                if !since_last.is_some_and(|gap| gap <= HOLD_RELEASE_GAP) {
                    self.started = Some(now);
                    return QuitPress::Hint;
                }
                if self
                    .started
                    .is_some_and(|started| now.saturating_duration_since(started) >= HOLD_TO_QUIT)
                {
                    self.started = None;
                    self.last = None;
                    QuitPress::Quit
                } else {
                    QuitPress::Wait
                }
            }
        }
    }
}

fn quit_shortcut(_: &QuitShortcut, cx: &mut App) {
    let gesture = crate::settings::quit_gesture(cx);
    match cx
        .default_global::<QuitPresses>()
        .press(gesture, Instant::now())
    {
        QuitPress::Quit => request_quit(cx),
        QuitPress::Hint if gesture == QuitGesture::Hold => {
            crate::toast::info(cx, "Keep holding ⌘Q to quit")
        }
        QuitPress::Hint => crate::toast::info(cx, "Press ⌘Q again to quit"),
        QuitPress::Wait => {}
    }
}

pub(crate) fn request_quit(cx: &mut App) {
    // Actions may arrive while GPUI has the active window borrowed. Inspect
    // roots only after that dispatch completes.
    cx.defer(prepare_quit);
}

fn prepare_quit(cx: &mut App) {
    let mut ready = true;
    for window in cx.windows() {
        if let Some(window) = window.downcast::<shell::Shell>() {
            ready &= window
                .update(cx, |shell, _, cx| shell.prepare_quit(cx))
                .unwrap_or(false);
        }
    }
    if ready {
        quit_after_save(cx);
    }
}

pub(crate) fn quit_after_save(cx: &mut App) {
    #[cfg(target_os = "macos")]
    native_quit::allow();
    cx.quit();
}

fn close_window(_: &CloseWindow, cx: &mut App) {
    cx.defer(close_active_window);
}

fn close_active_window(cx: &mut App) {
    if let Some(window) = cx.active_window() {
        if let Some(window) = window.downcast::<shell::Shell>() {
            window
                .update(cx, |shell, window, cx| {
                    // ⌘W closes the right pane's active surface first (the tab
                    // the user just opened); an empty or closed pane falls
                    // through to the window close, unsaved-file gate and all.
                    if shell.close_active_surface(window, cx) {
                        return;
                    }
                    if shell.prepare_window_close(cx) {
                        window.remove_window();
                    }
                })
                .ok();
        } else {
            window
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    }
}

/// Fixed app-level shortcuts backing the menu key equivalents. These live
/// outside the customizable keymap; `shell::apply_keymap` calls this after its
/// `clear_key_bindings` so they survive keymap re-application. Settings follows
/// the platform convention everywhere (Cmd+, on macOS, Ctrl+, elsewhere);
/// window/application lifecycle shortcuts remain macOS-only.
pub fn bind_keys(cx: &mut App) {
    let guarded_quit = crate::settings::quit_gesture(cx) != QuitGesture::Press;
    // Settings and Close tab are rebindable desktop commands.
    cx.bind_keys(
        app_key_bindings(cfg!(target_os = "macos"))
            .into_iter()
            .filter(|binding| {
                !binding.action().as_any().is::<shell::OpenSettings>()
                    && !binding.action().as_any().is::<CloseWindow>()
                    && !(guarded_quit && binding.action().as_any().is::<Quit>())
            }),
    );
    // A separate action, so clicking Quit in the menu still quits at once.
    if guarded_quit && cfg!(target_os = "macos") {
        cx.bind_keys([KeyBinding::new("cmd-q", QuitShortcut, None)]);
    }
}

/// The binding table behind [`bind_keys`] — `KeyBinding` construction is pure
/// (no `App`), so unit tests can inspect it directly.
fn app_key_bindings(macos: bool) -> Vec<KeyBinding> {
    let mut bindings = vec![KeyBinding::new(
        if macos { "cmd-," } else { "ctrl-," },
        shell::OpenSettings,
        None,
    )];
    if macos {
        bindings.extend([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
        ]);
    }
    bindings
}

/// The zuse menu bar. macOS renders this natively; mac-only entries are gated
/// at runtime (`cfg!`) so the whole module compiles and tests on Linux.
pub fn app_menus(cx: &App) -> Vec<Menu> {
    let localizer = cx
        .try_global::<crate::localization::Localization>()
        .map(|s| s.localizer.clone())
        .unwrap_or_default();
    localized_menus(&localizer)
}

fn localized_menus(l10n: &zuse_i18n::Localizer) -> Vec<Menu> {
    let macos = cfg!(target_os = "macos");

    // macOS titles the first menu with the bundle/process name regardless of
    // what we pass, but gpui still wants a name.
    let mut app_items = vec![
        // The native AppKit about panel; no equivalent elsewhere yet.
        MenuItem::action(l10n.text("native-about_zuse"), About).disabled(!macos),
        // Sparkle's placement: directly under About. Other platforms reach
        // the same check from the account menu.
        MenuItem::action(l10n.text("desktop-check_for_updates"), CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(
            l10n.text("commands-default_keybindings_settings"),
            shell::OpenSettings,
        ),
        MenuItem::separator(),
    ];
    if macos {
        app_items.extend([
            MenuItem::os_submenu(l10n.text("native-services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(l10n.text("native-hide_zuse"), Hide),
            MenuItem::action(l10n.text("native-hide_others"), HideOthers),
            MenuItem::action(l10n.text("native-show_all"), ShowAll),
            MenuItem::separator(),
        ]);
    }
    app_items.push(MenuItem::action(l10n.text("native-quit_zuse"), Quit));

    let mut menus = vec![
        Menu::new(l10n.text("common-startup_surface_zuse")).items(app_items),
        // Standard clipboard verbs tied to the composer's existing actions via
        // their native selectors (`OsAction` → cut:/copy:/paste:/selectAll:),
        // so the OS Edit menu routes through the responder chain to the focused
        // input — zed wires its editor actions identically
        // (crates/zed/src/zed/app_menus.rs, Edit/Selection menus).
        Menu::new(l10n.text("common-edit")).items([
            // Undo/Redo have no `OsAction` counterpart — they dispatch as plain
            // actions to the focused input, same as the composer keymap.
            MenuItem::action(l10n.text("native-undo"), composer::Undo),
            MenuItem::action(l10n.text("native-redo"), composer::Redo),
            MenuItem::separator(),
            MenuItem::os_action(l10n.text("native-cut"), composer::Cut, OsAction::Cut),
            MenuItem::os_action(
                l10n.text("chat-selection_menu_copy"),
                composer::Copy,
                OsAction::Copy,
            ),
            MenuItem::os_action(l10n.text("native-paste"), composer::Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action(
                l10n.text("native-select_all"),
                composer::SelectAll,
                OsAction::SelectAll,
            ),
        ]),
    ];
    // Appearance lives under View on every platform — it is the only View verb
    // today, but l10n.text("settings-settings_page_appearance") as a top-level menu would read oddly next to Edit.
    menus.push(Menu::new(l10n.text("desktop-view")).items([
        MenuItem::action(l10n.text("native-appearance_system"), AppearanceSystem),
        MenuItem::action(l10n.text("native-appearance_light"), AppearanceLight),
        MenuItem::action(l10n.text("native-appearance_dark"), AppearanceDark),
    ]));
    if macos {
        // Standard Window menu; macOS appends the open-window list itself.
        menus.push(Menu::new(l10n.text("desktop-window")).items([
            MenuItem::action(l10n.text("native-minimize"), Minimize),
            MenuItem::action(l10n.text("native-zoom"), Zoom),
            MenuItem::separator(),
            MenuItem::action(
                l10n.text("desktop-close_tab"),
                shell::DesktopCommand(crate::settings::keybindings::Command::CloseTab),
            ),
        ]));
    }
    menus
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Action as _, Keystroke};

    fn action_names(menu: &Menu) -> Vec<&'static str> {
        menu.items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { action, .. } => Some(action.name()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_single_press_quits_at_once_by_default() {
        let mut presses = QuitPresses::default();
        assert_eq!(
            presses.press(QuitGesture::Press, Instant::now()),
            QuitPress::Quit
        );
    }

    #[test]
    fn double_press_needs_two_presses_close_together() {
        let now = Instant::now();
        let mut presses = QuitPresses::default();
        assert_eq!(
            presses.press(QuitGesture::DoublePress, now),
            QuitPress::Hint
        );
        let too_late = now + DOUBLE_PRESS_WINDOW + Duration::from_millis(1);
        assert_eq!(
            presses.press(QuitGesture::DoublePress, too_late),
            QuitPress::Hint
        );
        assert_eq!(
            presses.press(
                QuitGesture::DoublePress,
                too_late + Duration::from_millis(300)
            ),
            QuitPress::Quit
        );
    }

    #[test]
    fn hold_quits_only_after_an_unbroken_hold() {
        let now = Instant::now();
        let at = |ms| now + Duration::from_millis(ms);
        let mut presses = QuitPresses::default();
        assert_eq!(presses.press(QuitGesture::Hold, at(0)), QuitPress::Hint);
        // The key starts repeating after the usual delay.
        assert_eq!(presses.press(QuitGesture::Hold, at(500)), QuitPress::Wait);
        assert_eq!(presses.press(QuitGesture::Hold, at(590)), QuitPress::Wait);
        assert_eq!(presses.press(QuitGesture::Hold, at(680)), QuitPress::Wait);
        assert_eq!(presses.press(QuitGesture::Hold, at(770)), QuitPress::Wait);
        assert_eq!(presses.press(QuitGesture::Hold, at(860)), QuitPress::Quit);
    }

    #[test]
    fn letting_go_restarts_the_hold() {
        let now = Instant::now();
        let at = |ms| now + Duration::from_millis(ms);
        let mut presses = QuitPresses::default();
        assert_eq!(presses.press(QuitGesture::Hold, at(0)), QuitPress::Hint);
        assert_eq!(presses.press(QuitGesture::Hold, at(2000)), QuitPress::Hint);
        assert_eq!(presses.press(QuitGesture::Hold, at(2100)), QuitPress::Wait);
    }

    #[test]
    fn app_menu_ends_with_quit() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        assert_eq!(menus[0].name.as_ref(), "Zuse");
        let Some(MenuItem::Action { name, action, .. }) = menus[0].items.last() else {
            panic!("last app-menu item must be an action");
        };
        assert_eq!(name.as_ref(), "Quit Zuse");
        assert_eq!(action.name(), Quit.name());
    }

    #[test]
    fn app_menu_offers_settings() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        assert!(
            menus[0].items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, action, .. }
                    if name.as_ref() == "Settings" && action.name() == shell::OpenSettings.name()
            )),
            "the application menu should expose Settings"
        );
    }

    #[test]
    fn about_enabled_only_on_macos() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        let Some(first @ MenuItem::Action { action, .. }) = menus[0].items.first() else {
            panic!("first app-menu item must be an action");
        };
        assert_eq!(action.name(), About.name());
        assert_eq!(first.is_disabled(), !cfg!(target_os = "macos"));
    }

    #[test]
    fn check_for_updates_follows_about() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        let Some(MenuItem::Action {
            name,
            action,
            disabled,
            ..
        }) = menus[0].items.get(1)
        else {
            panic!("second app-menu item must be an action");
        };
        assert_eq!(name.as_ref(), "Check for Updates…");
        assert_eq!(action.name(), CheckForUpdates.name());
        assert!(!disabled);
    }

    #[test]
    fn edit_menu_uses_composer_clipboard_os_actions() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        let edit = menus
            .iter()
            .find(|m| m.name.as_ref() == "Edit")
            .expect("Edit menu present");
        // `OsAction` has no `Debug` impl at the pinned rev, so compare
        // per-field.
        let expect = [
            (composer::Cut.name(), OsAction::Cut),
            (composer::Copy.name(), OsAction::Copy),
            (composer::Paste.name(), OsAction::Paste),
            (composer::SelectAll.name(), OsAction::SelectAll),
        ];
        let got: Vec<(&str, OsAction)> = edit
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action {
                    action,
                    os_action: Some(os_action),
                    ..
                } => Some((action.name(), *os_action)),
                _ => None,
            })
            .collect();
        assert_eq!(got.len(), expect.len());
        for ((got_name, got_os), (want_name, want_os)) in got.iter().zip(expect.iter()) {
            assert_eq!(got_name, want_name);
            assert!(got_os == want_os, "OsAction mismatch for {want_name}");
        }
    }

    #[test]
    fn view_menu_offers_all_three_appearance_modes() {
        let menus = localized_menus(&zuse_i18n::Localizer::default());
        let view = menus
            .iter()
            .find(|m| m.name.as_ref() == "View")
            .expect("View menu present");
        assert_eq!(
            action_names(view),
            vec![
                AppearanceSystem.name(),
                AppearanceLight.name(),
                AppearanceDark.name()
            ]
        );
    }

    #[test]
    fn app_bindings_use_platform_settings_convention() {
        // `KeyBinding::new` panics on unparseable combos, so constructing the
        // table is itself the parse check.
        let find = |bindings: &[KeyBinding], name: &str| {
            bindings
                .iter()
                .find(|binding| binding.action().name() == name)
                .map(|binding| {
                    binding
                        .keystrokes()
                        .iter()
                        .map(|ks| ks.inner().clone())
                        .collect::<Vec<_>>()
                })
        };
        let combo = |source: &str| vec![Keystroke::parse(source).unwrap()];
        let macos = app_key_bindings(true);
        assert_eq!(
            find(&macos, shell::OpenSettings.name()),
            Some(combo("cmd-,"))
        );
        assert_eq!(find(&macos, Quit.name()), Some(combo("cmd-q")));
        assert_eq!(find(&macos, CloseWindow.name()), Some(combo("cmd-w")));
        assert_eq!(find(&macos, Minimize.name()), Some(combo("cmd-m")));

        let other = app_key_bindings(false);
        assert_eq!(
            find(&other, shell::OpenSettings.name()),
            Some(combo("ctrl-,"))
        );
        assert_eq!(find(&other, Quit.name()), None);
    }
}

// Standard AppKit about panel. Icon and copyright still come from the bundle's
// Info.plist; name and version are passed explicitly so unbundled dev builds
// (`cargo run`) show "Zuse" and the crate version instead of the process name.
#[cfg(target_os = "macos")]
mod about_panel {
    use objc2::MainThreadMarker;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
        NSAboutPanelOptionVersion, NSApplication,
    };
    use objc2_foundation::{NSDictionary, NSString};

    pub(super) fn show() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let name = NSString::from_str("Zuse");
        let version = NSString::from_str(env!("CARGO_PKG_VERSION"));
        // Empty build version: CFBundleVersion equals the marketing version, and
        // AppKit would otherwise render "Version 0.2.61 (0.2.61)".
        let build = NSString::from_str("");
        let values: [&AnyObject; 3] = [&name, &version, &build];
        unsafe {
            let options = NSDictionary::from_slices(
                &[
                    NSAboutPanelOptionApplicationName,
                    NSAboutPanelOptionApplicationVersion,
                    NSAboutPanelOptionVersion,
                ],
                &values,
            );
            NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&options);
        }
    }
}

// GPUI's on_app_quit runs after AppKit has committed to termination. Catch
// Dock / system Quit at the earlier delegate decision, just like our menu
// action, so failed saves can still cancel termination. The pinned GPUI
// delegate does not implement applicationShouldTerminate:.
#[cfg(target_os = "macos")]
mod native_quit {
    use super::*;
    use objc::runtime::{Class, Object, Sel, class_addMethod};
    use objc::{class, msg_send, sel, sel_impl};
    use std::cell::{Cell, RefCell};

    thread_local! {
        static ALLOWED: Cell<bool> = const { Cell::new(false) };
        static REQUEST: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    extern "C" fn should_terminate(_: &Object, _: Sel, _: *mut Object) -> usize {
        if ALLOWED.get() {
            return 1; // NSTerminateNow
        }
        REQUEST.with_borrow(|request| {
            if let Some(request) = request {
                request();
            }
        });
        0 // NSTerminateCancel; re-request only after buffers are safe.
    }

    pub(super) fn allow() {
        ALLOWED.set(true);
    }

    pub(super) fn init(cx: &mut App) {
        let executor = cx.foreground_executor().clone();
        let app = cx.to_async();
        REQUEST.with_borrow_mut(|request| {
            *request = Some(Box::new(move || {
                let app = app.clone();
                executor
                    .spawn(async move {
                        let _ = app.update(request_quit);
                    })
                    .detach();
            }));
        });
        // AppKit invokes delegate methods on the main thread. Add only the
        // missing selector: never replace GPUI's existing quit observer.
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut Object = msg_send![app, delegate];
            assert!(!delegate.is_null(), "GPUI application delegate must exist");
            let class = (*delegate).class() as *const Class as *mut Class;
            let added = class_addMethod(
                class,
                sel!(applicationShouldTerminate:),
                std::mem::transmute::<
                    extern "C" fn(&Object, Sel, *mut Object) -> usize,
                    unsafe extern "C" fn(),
                >(should_terminate),
                c"Q@:@".as_ptr(),
            );
            assert!(
                added == objc::runtime::YES,
                "GPUI termination delegate contract changed"
            );
        }
    }
}
