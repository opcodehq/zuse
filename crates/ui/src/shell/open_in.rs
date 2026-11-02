//! The titlebar's "Open in" split button: open the chat's workspace in an
//! app installed on this computer, or copy its path.
//!
//! Detection follows the desktop host: a fixed target list. macOS checks
//! each app's known bundle paths and converts its bundle icon to a cached
//! PNG with `sips`; Linux and Windows look the editors and terminals up on
//! PATH and draw fallback glyphs. The file manager is always offered.
use crate::localization::message as tr;
use std::path::{Path, PathBuf};
use zuse_i18n::messages as msg;

use super::*;

/// One app the workspace can open in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenTarget {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    /// macOS: the app bundle (or, unresolved, its name) for `open -a`.
    /// Elsewhere: the executable resolved on PATH. `None` reveals the path
    /// in the file manager.
    launcher: Option<PathBuf>,
    /// A PNG of the app's own icon (macOS only).
    pub(super) icon: Option<PathBuf>,
}

/// The menu's state: its popup and the last detection.
#[derive(Default)]
pub(super) struct OpenIn {
    pub(super) menu: popover::Popup<()>,
    pub(super) targets: Vec<OpenTarget>,
    pub(super) loading: bool,
    detected: bool,
    task: Option<Task<()>>,
}

struct TargetDefinition {
    id: &'static str,
    label: &'static str,
    app_name: Option<&'static str>,
    /// Bundle paths, `~` meaning the home directory.
    app_paths: &'static [&'static str],
    icon_names: &'static [&'static str],
    icon_paths: &'static [&'static str],
}

const MAC_TARGETS: &[TargetDefinition] = &[
    TargetDefinition {
        id: "finder",
        label: "Finder",
        app_name: None,
        app_paths: &["/System/Library/CoreServices/Finder.app"],
        icon_names: &["Finder"],
        icon_paths: &[
            "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources/FinderIcon.icns",
        ],
    },
    TargetDefinition {
        id: "cursor",
        label: "Cursor",
        app_name: Some("Cursor"),
        app_paths: &["/Applications/Cursor.app", "~/Applications/Cursor.app"],
        icon_names: &["Cursor"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "vscode",
        label: "VS Code",
        app_name: Some("Visual Studio Code"),
        app_paths: &[
            "/Applications/Visual Studio Code.app",
            "~/Applications/Visual Studio Code.app",
            "/Applications/Visual Studio Code - Insiders.app",
            "~/Applications/Visual Studio Code - Insiders.app",
        ],
        icon_names: &["Code", "Visual Studio Code", "VSCode"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "windsurf",
        label: "Windsurf",
        app_name: Some("Windsurf"),
        app_paths: &["/Applications/Windsurf.app", "~/Applications/Windsurf.app"],
        icon_names: &["Windsurf"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "zed",
        label: "Zed",
        app_name: Some("Zed"),
        app_paths: &["/Applications/Zed.app", "~/Applications/Zed.app"],
        icon_names: &["Zed"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "xcode",
        label: "Xcode",
        app_name: Some("Xcode"),
        app_paths: &["/Applications/Xcode.app", "~/Applications/Xcode.app"],
        icon_names: &["Xcode"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "ghostty",
        label: "Ghostty",
        app_name: Some("Ghostty"),
        app_paths: &["/Applications/Ghostty.app", "~/Applications/Ghostty.app"],
        icon_names: &["Ghostty"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "terminal",
        label: "Terminal",
        app_name: Some("Terminal"),
        app_paths: &["/System/Applications/Utilities/Terminal.app"],
        icon_names: &["Terminal"],
        icon_paths: &[],
    },
    TargetDefinition {
        id: "antigravity",
        label: "Antigravity",
        app_name: Some("Antigravity"),
        app_paths: &[
            "/Applications/Antigravity.app",
            "~/Applications/Antigravity.app",
        ],
        icon_names: &["Antigravity"],
        icon_paths: &[],
    },
];

/// `(id, label, command)`; no command reveals in the file manager.
const LINUX_TARGETS: &[(&str, &str, Option<&str>)] = &[
    ("finder", "Files", None),
    ("cursor", "Cursor", Some("cursor")),
    ("vscode", "VS Code", Some("code")),
    ("windsurf", "Windsurf", Some("windsurf")),
    ("zed", "Zed", Some("zed")),
    ("ghostty", "Ghostty", Some("ghostty")),
    ("terminal", "Terminal", Some("x-terminal-emulator")),
];

const WINDOWS_TARGETS: &[(&str, &str, Option<&str>)] = &[
    ("finder", "File Explorer", None),
    ("cursor", "Cursor", Some("cursor")),
    ("vscode", "VS Code", Some("code")),
    ("windsurf", "Windsurf", Some("windsurf")),
    ("terminal", "Windows Terminal", Some("wt")),
];

fn home_path(path: &str) -> Option<PathBuf> {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(|home| Path::new(&home).join(rest)),
        None => Some(PathBuf::from(path)),
    }
}

fn first_existing(paths: &[&str]) -> Option<PathBuf> {
    paths
        .iter()
        .filter_map(|path| home_path(path))
        .find(|path| path.exists())
}

/// Resolve `command` on PATH (with PATHEXT on Windows).
fn executable_on_path(command: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path_var?)
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |ext| dir.join(format!("{command}{ext}")))
        })
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The PATH targets (Linux, Windows): the file manager always, the rest
/// when their command resolves.
fn portable_targets(
    definitions: &[(&'static str, &'static str, Option<&'static str>)],
    path_var: Option<&std::ffi::OsStr>,
) -> Vec<OpenTarget> {
    definitions
        .iter()
        .filter_map(|(id, label, command)| {
            let launcher = match command {
                None => None,
                Some(command) => Some(executable_on_path(command, path_var)?),
            };
            Some(OpenTarget {
                id,
                label,
                launcher,
                icon: None,
            })
        })
        .collect()
}

fn normalize_icon_hint(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

fn icon_file_names(name: &str) -> Vec<String> {
    let name = name.trim();
    if name.is_empty() {
        Vec::new()
    } else if name.to_lowercase().ends_with(".icns") {
        vec![name.to_owned()]
    } else {
        vec![name.to_owned(), format!("{name}.icns")]
    }
}

/// The `.icns` among `entries` that best matches the app's names: exact
/// name 100, partial 80, generic document icons −50. Falls back to the
/// first `.icns` when nothing scores.
fn best_icns(entries: &[String], hints: &[String]) -> Option<String> {
    let generic = ["document", "default", "file", "text"];
    let mut scored: Vec<(i32, &String)> = entries
        .iter()
        .filter(|entry| entry.to_lowercase().ends_with(".icns"))
        .map(|entry| {
            let stem = &entry[..entry.len() - ".icns".len()];
            let base = normalize_icon_hint(stem);
            let mut score = 0;
            if hints.contains(&base) {
                score += 100;
            } else if hints
                .iter()
                .any(|hint| !hint.is_empty() && (base.contains(hint) || hint.contains(&base)))
            {
                score += 80;
            }
            if generic.contains(&base.as_str()) {
                score -= 50;
            }
            (score, entry)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored
        .iter()
        .find(|(score, _)| *score > 0)
        .or(scored.first())
        .map(|(_, entry)| (*entry).clone())
}

fn plist_value(plist: &Path, key: &str) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !value.is_empty()).then_some(value)
}

fn bundle_icon_path(target: &TargetDefinition, app: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = first_existing(target.icon_paths) {
        return Some(explicit);
    }
    let app = app?;
    let resources = app.join("Contents/Resources");
    let plist = app.join("Contents/Info.plist");
    let declared = ["CFBundleIconFile", "CFBundleIconName"]
        .iter()
        .filter_map(|key| plist_value(&plist, key))
        .flat_map(|name| icon_file_names(&name));
    let named = target
        .icon_names
        .iter()
        .flat_map(|name| icon_file_names(name));
    if let Some(found) = declared
        .chain(named)
        .map(|name| resources.join(name))
        .find(|candidate| candidate.is_file())
    {
        return Some(found);
    }
    let entries: Vec<String> = std::fs::read_dir(&resources)
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    let hints: Vec<String> = [target.label, target.app_name.unwrap_or(""), target.id]
        .iter()
        .filter(|hint| !hint.is_empty())
        .map(|hint| normalize_icon_hint(hint))
        .collect();
    best_icns(&entries, &hints).map(|entry| resources.join(entry))
}

/// Convert the app's `.icns` to a PNG under `cache_dir`, keyed by the
/// source's size and mtime so an app update refreshes it.
fn cached_icon_png(
    target: &TargetDefinition,
    app: Option<&Path>,
    cache_dir: &Path,
) -> Option<PathBuf> {
    let icns = bundle_icon_path(target, app)?;
    let meta = std::fs::metadata(&icns).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis());
    let png = cache_dir.join(format!("{}-{}-{mtime}.png", target.id, meta.len()));
    if !png.is_file() {
        std::fs::create_dir_all(cache_dir).ok()?;
        let status = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(&icns)
            .arg("--out")
            .arg(&png)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
    }
    png.is_file().then_some(png)
}

fn mac_targets(cache_dir: &Path) -> Vec<OpenTarget> {
    MAC_TARGETS
        .iter()
        .filter_map(|target| {
            let app = first_existing(target.app_paths);
            let always = matches!(target.id, "finder" | "terminal");
            if app.is_none() && !always {
                return None;
            }
            let launcher = (target.id != "finder")
                .then(|| app.clone().or_else(|| target.app_name.map(PathBuf::from)))
                .flatten();
            Some(OpenTarget {
                id: target.id,
                label: target.label,
                launcher,
                icon: cached_icon_png(target, app.as_deref(), cache_dir),
            })
        })
        .collect()
}

/// The apps available on this computer, in menu order. Blocking: run it off
/// the UI thread.
pub(super) fn detect_targets(cache_dir: &Path) -> Vec<OpenTarget> {
    if cfg!(target_os = "macos") {
        mac_targets(cache_dir)
    } else if cfg!(windows) {
        portable_targets(WINDOWS_TARGETS, std::env::var_os("PATH").as_deref())
    } else {
        portable_targets(LINUX_TARGETS, std::env::var_os("PATH").as_deref())
    }
}

/// The command that opens `path` with `target`, or `None` to reveal it in
/// the file manager.
fn open_command(target: &OpenTarget, path: &str) -> Option<std::process::Command> {
    let launcher = target.launcher.as_ref()?;
    let mut command;
    if cfg!(target_os = "macos") {
        command = std::process::Command::new("open");
        command.arg("-a").arg(launcher).arg(path);
    } else {
        command = std::process::Command::new(launcher);
        match (target.id, cfg!(windows)) {
            ("terminal", true) => {
                command.args(["-d", path]);
            }
            ("terminal", false) => {
                command.args([
                    "-e",
                    "sh",
                    "-c",
                    "cd -- \"$1\" && exec \"${SHELL:-/bin/sh}\" -l",
                    "zuse-terminal",
                    path,
                ]);
            }
            ("ghostty", _) => {
                command.arg(format!("--working-directory={path}"));
            }
            _ => {
                command.arg(path);
            }
        }
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    Some(command)
}

/// The fallback glyph for an app without its own icon.
fn fallback_glyph(id: &str) -> &'static str {
    match id {
        "finder" => icons::FOLDER_OPEN,
        "ghostty" | "terminal" => icons::TERMINAL,
        _ => icons::ACTION_LINT,
    }
}

impl Shell {
    /// The selected chat's workspace path, and whether it is on this
    /// computer (only then can a local app open it).
    pub(super) fn open_in_path(&self, cx: &App) -> Option<(String, bool)> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        let path = chat_copy_path(chat)?.to_owned();
        let local = state.local_device_id.as_deref() == Some(chat.device_id.as_str());
        Some((path, local))
    }

    fn open_in_icon_cache(&self) -> PathBuf {
        self.data_dir.join("open-target-icons")
    }

    /// Detect the apps again (the menu refreshes on every open).
    fn refresh_open_targets(&mut self, cx: &mut Context<Self>) {
        if self.open_in.loading {
            return;
        }
        self.open_in.loading = !self.open_in.detected;
        let cache = self.open_in_icon_cache();
        let detect = cx
            .background_executor()
            .spawn(async move { detect_targets(&cache) });
        self.open_in.task = Some(cx.spawn(async move |this, cx| {
            let targets = detect.await;
            this.update(cx, |this, cx| {
                this.open_in.targets = targets;
                this.open_in.loading = false;
                this.open_in.detected = true;
                this.open_in.task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn ensure_open_targets(&mut self, cx: &mut Context<Self>) {
        if !self.open_in.detected && self.open_in.task.is_none() {
            self.refresh_open_targets(cx);
        }
    }

    pub(super) fn close_open_in(&mut self, cx: &mut Context<Self>) {
        if self.open_in.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.open_in.menu);
        }
        cx.notify();
    }

    /// The targets the menu lists: none for a workspace on another device.
    fn open_in_menu_targets(&self, cx: &App) -> Vec<OpenTarget> {
        match self.open_in_path(cx) {
            Some((_, true)) => self.open_in.targets.clone(),
            _ => Vec::new(),
        }
    }

    /// Open the chat's workspace in `target`, off the UI thread.
    pub(super) fn open_workspace_in(&mut self, target: OpenTarget, cx: &mut Context<Self>) {
        self.close_open_in(cx);
        let Some((path, true)) = self.open_in_path(cx) else {
            return;
        };
        let Some(mut command) = open_command(&target, &path) else {
            cx.reveal_path(Path::new(&path));
            return;
        };
        let label = target.label;
        // `open -a` exits once the app has the path; an editor launched
        // directly keeps running, so a detached thread reaps it.
        let wait_for_exit = cfg!(target_os = "macos");
        let launch = cx.background_executor().spawn(async move {
            let mut child = command.spawn().map_err(|error| error.to_string())?;
            if wait_for_exit {
                let status = child.wait().map_err(|error| error.to_string())?;
                if !status.success() {
                    return Err(format!("exited with {status}"));
                }
            } else {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Ok::<(), String>(())
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = launch.await {
                tracing::warn!(target = label, %error, "open workspace in app failed");
                this.update(cx, |this, cx| {
                    this.show_notice(
                        crate::toast::ToastKind::Error,
                        format!("Couldn't open in {label}"),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn copy_open_in_path(&mut self, cx: &mut Context<Self>) {
        self.close_open_in(cx);
        if let Some((path, _)) = self.open_in_path(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(path));
            self.show_notice(crate::toast::ToastKind::Success, "Path copied", cx);
        }
    }

    /// Keys while the menu is open: 1…N opens that app, ⌘⇧C copies the
    /// path, Escape closes. True when the key was the menu's.
    pub(super) fn open_in_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.open_in.menu.is_open() {
            return false;
        }
        let keystroke = &event.keystroke;
        let mods = keystroke.modifiers;
        if keystroke.key == "escape" {
            self.close_open_in(cx);
            return true;
        }
        if keystroke.key == "c" && mods.shift && (mods.platform || mods.control) {
            self.copy_open_in_path(cx);
            return true;
        }
        if mods.platform || mods.control || mods.alt {
            return false;
        }
        let Some(index) = keystroke
            .key
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(|n| n - 1)
        else {
            return false;
        };
        match self.open_in_menu_targets(cx).get(index).cloned() {
            Some(target) => {
                self.open_workspace_in(target, cx);
                true
            }
            None => false,
        }
    }

    fn open_target_icon(target: &OpenTarget, theme: &Theme) -> AnyElement {
        match &target.icon {
            Some(png) => gpui::img(png.clone())
                .size(px(20.0))
                .flex_none()
                .rounded(px(4.0))
                .into_any_element(),
            None => icon(fallback_glyph(target.id))
                .size(px(20.0))
                .flex_none()
                .text_color(theme.text_muted)
                .into_any_element(),
        }
    }

    /// The split trigger (file manager icon │ chevron) and, while open, its
    /// menu. `None` on the new-session canvas.
    pub(super) fn render_open_in(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, local) = self.open_in_path(cx)?;
        self.ensure_open_targets(cx);
        let open = self.open_in.menu.get().is_some();
        let primary = self
            .open_in
            .targets
            .iter()
            .find(|t| t.id == "finder")
            .cloned();
        let primary_glyph = match (&primary, self.open_in.loading) {
            (_, true) => {
                loaders::mini_glyph_spinner("open-in-loading", 2.0, theme.glyph, cx.entity_id(), cx)
                    .into_any_element()
            }
            (Some(primary), false) if local => Self::open_target_icon(primary, theme),
            _ => icon(icons::ARROW_UP_RIGHT)
                .size(px(14.0))
                .text_color(theme.text_muted)
                .into_any_element(),
        };
        let border = theme.border_strong.opacity(0.8);
        let fade = "open-in-trigger";
        let mut trigger = div()
            .id("open-in-trigger")
            .debug_selector(|| "open-in-trigger".into())
            .role(gpui::Role::Button)
            .aria_label(tr(cx, msg::chat::top_bar_open_workspace_in_app))
            .aria_expanded(open)
            .h(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .rounded(px(6.0))
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .cursor_pointer()
            .text_color(theme.text_muted)
            .bg(if open {
                crate::theme::wash(0.10)
            } else {
                motion::hover_blend(fade, crate::theme::wash(0.0), crate::theme::wash(0.05))
            })
            .on_hover(motion::hover_listener(fade))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.open_in.menu.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.open_in.menu.take_press_was_open() {
                    this.close_open_in(cx);
                } else {
                    this.refresh_open_targets(cx);
                    this.open_in.menu.open(());
                    cx.notify();
                }
            }))
            .tooltip(crate::settings::widgets::text_tooltip("Open in…"))
            .child(
                div()
                    .size(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_r_1()
                    .border_color(border)
                    .child(primary_glyph),
            )
            .child(
                div()
                    .size(px(26.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(icons::ALT_ARROW_DOWN)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    ),
            );
        if open {
            let closing = self.open_in.menu.closing_since();
            let menu = self.render_open_in_menu(theme, local, cx);
            trigger = trigger.relative().child(popover::anchored_menu_below_end(
                "open-in-menu",
                menu,
                closing,
            ));
        }
        Some(trigger.into_any_element())
    }

    fn render_open_in_menu(
        &mut self,
        theme: &Theme,
        local: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme.for_popup();
        let targets = self.open_in_menu_targets(cx);
        let rows = targets.into_iter().enumerate().map(|(index, target)| {
            let fade = format!("open-in-{}", target.id);
            let label = target.label;
            let glyph = Self::open_target_icon(&target, &theme);
            popover::menu_row(&theme, false, fade.clone())
                .id(SharedString::from(fade.clone()))
                .debug_selector(move || fade.clone())
                .role(gpui::Role::MenuItem)
                .aria_label(label)
                .gap(px(12.0))
                .text_size(crate::typography::text_md())
                .child(glyph)
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(popover::kbd_hint(&theme, &(index + 1).to_string()))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_workspace_in(target.clone(), cx)),
                )
        });
        let copy_shortcut = crate::settings::badge_combo("mod-shift-c");
        popover::popover_card(&theme)
            .min_w(px(224.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_open_in(cx)))
            .flex()
            .flex_col()
            .children(rows)
            .when(!local, |card| {
                card.child(
                    div()
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(tr(cx, msg::native::this_workspace_is_on_another_computer)),
                )
            })
            .child(popover::menu_separator())
            .child(
                popover::menu_row(&theme, false, "open-in-copy-path")
                    .id("open-in-copy-path")
                    .debug_selector(|| "open-in-copy-path".into())
                    .role(gpui::Role::MenuItem)
                    .aria_label(tr(cx, msg::chat::top_bar_copy_path))
                    .text_size(crate::typography::text_md())
                    .child(
                        icon(icons::COPY)
                            .size(px(16.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(tr(cx, msg::chat::top_bar_copy_path)),
                    )
                    .child(popover::kbd_hint(&theme, &copy_shortcut))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_open_in_path(cx))),
            )
            .into_any_element()
    }
}

#[cfg(feature = "browser-fixture")]
impl Shell {
    /// A fixed target list for review captures (no detection, no icons).
    pub(super) fn fixture_open_in_menu(&mut self, cx: &mut Context<Self>) {
        self.open_in.targets = LINUX_TARGETS
            .iter()
            .map(|(id, label, command)| OpenTarget {
                id,
                label,
                launcher: command.map(PathBuf::from),
                icon: None,
            })
            .collect();
        self.open_in.detected = true;
        self.open_in.loading = false;
        self.open_in.menu.open(());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_targets_keep_the_file_manager_and_resolve_commands() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("zed");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        let targets = portable_targets(LINUX_TARGETS, Some(path.as_os_str()));
        let ids: Vec<_> = targets.iter().map(|t| t.id).collect();
        assert_eq!(ids, ["finder", "zed"]);
        assert_eq!(targets[0].label, "Files");
        assert_eq!(targets[1].launcher.as_deref(), Some(bin.as_path()));
        // Nothing on PATH still offers the file manager.
        let ids: Vec<_> = portable_targets(WINDOWS_TARGETS, None)
            .iter()
            .map(|t| t.label)
            .collect();
        assert_eq!(ids, ["File Explorer"]);
    }

    #[test]
    fn icon_scoring_prefers_the_app_name_over_documents() {
        let entries = ["document.icns", "AppIcon.icns", "Zed.icns", "readme.txt"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            best_icns(&entries, &["zed".into()]).as_deref(),
            Some("Zed.icns")
        );
        assert_eq!(
            best_icns(&entries, &["cursor".into()]).as_deref(),
            Some("AppIcon.icns")
        );
        assert_eq!(icon_file_names("Code"), ["Code", "Code.icns"]);
        assert_eq!(icon_file_names("x.icns"), ["x.icns"]);
    }

    #[test]
    fn commands_open_the_workspace_with_each_app() {
        let target = |id: &'static str, launcher: Option<&str>| OpenTarget {
            id,
            label: id,
            launcher: launcher.map(PathBuf::from),
            icon: None,
        };
        assert!(open_command(&target("finder", None), "/w").is_none());
        let command = open_command(&target("zed", Some("/bin/zed")), "/w").unwrap();
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "macos") {
            assert_eq!(command.get_program(), "open");
            assert_eq!(args, ["-a", "/bin/zed", "/w"]);
        } else {
            assert_eq!(command.get_program(), "/bin/zed");
            assert_eq!(args, ["/w"]);
        }
    }
}
