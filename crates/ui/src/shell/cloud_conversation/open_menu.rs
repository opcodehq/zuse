//! The app menus that float over the computer menu: the synced folder's
//! apps and the ways in over SSH. One component serves both: a numbered
//! list under a split button's chevron, drawn on top of the computer menu
//! without replacing it. Arrows move, Enter or a number picks, Escape or a
//! click outside closes only this menu.
use super::computer::{SshAction, ssh_entries};
use super::*;
use crate::shell::open_in::OpenTarget;
use crate::shell::ssh_open::SshTarget;

/// Which split button the menu belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Folder,
    Ssh,
}

/// The open menu: which one, its keyboard row, and whether Delete folder
/// is waiting for its second press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Nested {
    pub(super) kind: Kind,
    pub(super) highlighted: Option<usize>,
    pub(super) confirm_delete: bool,
}

/// What a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// Open the synced folder in an app (and make it the default).
    Folder(OpenTarget),
    CopyPath,
    /// Stop sync if on and delete the kept folder.
    DeleteFolder,
    Ssh(SshAction),
}

/// One row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub(super) id: String,
    pub(super) label: &'static str,
    pub(super) pick: Pick,
    /// The number key that picks it.
    pub(super) number: Option<usize>,
    /// A shortcut that works while the menu is open, as a key combo.
    pub(super) shortcut: Option<&'static str>,
    pub(super) danger: bool,
    /// A hairline above the row.
    pub(super) separated: bool,
}

impl Item {
    fn new(id: impl Into<String>, label: &'static str, pick: Pick) -> Self {
        Self {
            id: id.into(),
            label,
            pick,
            number: None,
            shortcut: None,
            danger: false,
            separated: false,
        }
    }
}

/// Copy path's shortcut, the same as the titlebar's Open in menu.
pub(super) const COPY_PATH_SHORTCUT: &str = "mod-shift-c";

/// Number the rows 1 to 9 in order. A destructive row never gets a number,
/// so a stray key can't delete anything.
fn numbered(mut items: Vec<Item>) -> Vec<Item> {
    let mut next = 1;
    for item in &mut items {
        if !item.danger && next <= 9 {
            item.number = Some(next);
            next += 1;
        }
    }
    items
}

/// The synced folder's menu: every installed app, Copy path, then Delete
/// folder while sync is off and the folder is kept.
pub(super) fn folder_items(targets: &[OpenTarget], deletable: bool) -> Vec<Item> {
    let mut items: Vec<Item> = targets
        .iter()
        .map(|target| {
            Item::new(
                format!("cloud-folder-open-{}", target.id),
                target.label,
                Pick::Folder(target.clone()),
            )
        })
        .collect();
    let mut copy = Item::new("cloud-folder-copy-path", "Copy path", Pick::CopyPath);
    copy.shortcut = Some(COPY_PATH_SHORTCUT);
    copy.separated = !items.is_empty();
    items.push(copy);
    if deletable {
        let mut delete = Item::new("cloud-folder-delete", "Delete folder", Pick::DeleteFolder);
        delete.danger = true;
        delete.separated = true;
        items.push(delete);
    }
    numbered(items)
}

/// The SSH menu: every app that opens the computer over SSH (the system
/// terminal until detection finishes), then Copy SSH command.
pub(super) fn ssh_items(targets: &[SshTarget]) -> Vec<Item> {
    let entries = ssh_entries(targets);
    let count = entries.len();
    numbered(
        entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let mut item = Item::new(
                    format!("cloud-ssh-open-{}", entry.id()),
                    entry.label(),
                    Pick::Ssh(entry.action()),
                );
                item.separated = index + 1 == count && count > 1;
                item
            })
            .collect(),
    )
}

/// The app the folder button opens in: the saved choice while it is still
/// installed, else the file manager, else the first app.
pub(super) fn default_target(targets: &[OpenTarget], saved: Option<&str>) -> Option<OpenTarget> {
    saved
        .and_then(|id| targets.iter().find(|target| target.id == id))
        .or_else(|| targets.iter().find(|target| target.id == "finder"))
        .or_else(|| targets.first())
        .cloned()
}

/// `path` with the home folder written as `~`.
pub(super) fn tilde(path: &str, home: Option<&str>) -> String {
    match home.map(|home| home.trim_end_matches('/')) {
        Some(home) if !home.is_empty() => match path.strip_prefix(home) {
            Some("") => "~".to_owned(),
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => path.to_owned(),
        },
        _ => path.to_owned(),
    }
}

/// A folder path cut to `max` characters in the middle: an ellipsis in
/// place of the folders between its root and as many of its last folders
/// as fit (a folder's own name matters most). The root stays unless
/// dropping it keeps one more folder. A path whose last folder alone is
/// too long loses characters from its middle instead.
pub(super) fn middle_truncate(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 3 {
        return text.to_owned();
    }
    let parts: Vec<&str> = text.split('/').collect();
    // "~/a/b" keeps "~"; "/Users/a/b" keeps "/Users".
    let root = if parts[0].is_empty() { 2 } else { 1 };
    // The last folders that fit after `lead`.
    let tail_after = |lead: &str| -> Vec<&str> {
        let mut tail = Vec::new();
        let mut length = lead.chars().count();
        for part in parts.iter().skip(root).rev() {
            length += 1 + part.chars().count();
            if length > max {
                break;
            }
            tail.insert(0, *part);
        }
        tail
    };
    if parts.len() > root + 1 {
        let head = format!("{}/…", parts[..root].join("/"));
        let with_root = tail_after(&head);
        let without_root = tail_after("…");
        if without_root.len() > with_root.len() {
            return format!("…/{}", without_root.join("/"));
        }
        if !with_root.is_empty() {
            return format!("{head}/{}", with_root.join("/"));
        }
    }
    let keep = max - 1;
    let tail = keep.div_ceil(5) * 3;
    let tail = tail.min(keep);
    let head = keep - tail;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// What a key does to the open menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyOutcome {
    Close,
    Highlight(usize),
    Pick(usize),
    /// Not the menu's key: let it through.
    Ignore,
}

/// Arrows move (wrapping), Enter picks the highlighted row, a number picks
/// its row, the row's shortcut picks it, Escape closes.
pub(super) fn key_outcome(
    items: &[Item],
    highlighted: Option<usize>,
    key: &str,
    modifiers: gpui::Modifiers,
) -> KeyOutcome {
    let combo_mod = modifiers.platform || modifiers.control;
    if let Some(index) = items.iter().position(|item| {
        item.shortcut == Some(COPY_PATH_SHORTCUT) && key == "c" && modifiers.shift && combo_mod
    }) {
        return KeyOutcome::Pick(index);
    }
    if combo_mod || modifiers.alt {
        return KeyOutcome::Ignore;
    }
    match popover::classify_key(key, false, false) {
        popover::MenuKey::Escape => return KeyOutcome::Close,
        popover::MenuKey::Up => {
            return popover::menu_step(highlighted, items.len(), -1)
                .map_or(KeyOutcome::Ignore, KeyOutcome::Highlight);
        }
        popover::MenuKey::Down => {
            return popover::menu_step(highlighted, items.len(), 1)
                .map_or(KeyOutcome::Ignore, KeyOutcome::Highlight);
        }
        popover::MenuKey::Enter => {
            return highlighted
                .filter(|index| *index < items.len())
                .map_or(KeyOutcome::Ignore, KeyOutcome::Pick);
        }
        _ => {}
    }
    key.parse::<usize>()
        .ok()
        .and_then(|number| items.iter().position(|item| item.number == Some(number)))
        .map_or(KeyOutcome::Ignore, KeyOutcome::Pick)
}

impl CloudConversation {
    /// The rows of `kind`'s menu as things stand.
    fn nested_items(&self, kind: Kind) -> Vec<Item> {
        match kind {
            Kind::Folder => folder_items(&self.computer.open_targets, self.checkout_deletable()),
            Kind::Ssh => ssh_items(&self.computer.ssh_targets),
        }
    }

    /// Open `kind`'s menu, or close it when the press that led here found
    /// it open.
    pub(super) fn toggle_nested(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.computer.nested.take_press_was_open() {
            self.close_nested(cx);
            return;
        }
        let highlighted = match kind {
            Kind::Folder => {
                let default = self.folder_app(cx).map(|target| target.id);
                self.nested_items(kind)
                    .iter()
                    .position(|item| matches!(&item.pick, Pick::Folder(t) if Some(t.id) == default))
                    .or(Some(0))
            }
            Kind::Ssh => Some(0),
        };
        self.computer.nested.open(Nested {
            kind,
            highlighted,
            confirm_delete: false,
        });
        cx.emit(CloudConversationEvent::RefreshOpenTargets);
        cx.notify();
    }

    /// Review captures: `FIXTURE_NESTED=folder|ssh` opens that app menu
    /// along with the computer menu.
    #[cfg(feature = "appshots-fixture")]
    pub(super) fn open_fixture_nested(&mut self, cx: &mut Context<Self>) {
        let kind = match std::env::var("FIXTURE_NESTED").as_deref() {
            Ok("folder") => Kind::Folder,
            Ok("ssh") => Kind::Ssh,
            _ => return,
        };
        self.toggle_nested(kind, cx);
    }

    pub(super) fn close_nested(&mut self, cx: &mut Context<Self>) {
        self.computer.nested = Default::default();
        self.computer.nested_bounds = None;
        cx.notify();
    }

    /// Whether the menu of `kind` is the one open.
    pub(super) fn nested_open(&self, kind: Kind) -> bool {
        self.computer
            .nested
            .get()
            .is_some_and(|menu| menu.kind == kind)
    }

    /// Record, from a chevron's mouse-down, whether its menu was open.
    pub(super) fn note_nested_press(&mut self, kind: Kind) {
        self.computer
            .nested
            .note_trigger_press_matching(|menu| menu.kind == kind);
    }

    /// Keys while the computer menu is open: the app menu's first, then
    /// Escape closes the computer menu. True when the key was taken.
    pub(in crate::shell) fn computer_menu_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.computer.open {
            return false;
        }
        let keystroke = &event.keystroke;
        let Some(menu) = self.computer.nested.as_open().cloned() else {
            if keystroke.key == "escape" {
                self.close_computer(cx);
                return true;
            }
            return false;
        };
        let items = self.nested_items(menu.kind);
        match key_outcome(
            &items,
            menu.highlighted,
            &keystroke.key,
            keystroke.modifiers,
        ) {
            KeyOutcome::Close => self.close_nested(cx),
            KeyOutcome::Highlight(index) => {
                if let Some(menu) = self.computer.nested.open_mut() {
                    menu.highlighted = Some(index);
                    menu.confirm_delete = false;
                }
                cx.notify();
            }
            KeyOutcome::Pick(index) => self.pick_nested(index, cx),
            KeyOutcome::Ignore => return false,
        }
        true
    }

    fn pick_nested(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(menu) = self.computer.nested.as_open().cloned() else {
            return;
        };
        let Some(item) = self.nested_items(menu.kind).into_iter().nth(index) else {
            return;
        };
        match item.pick {
            Pick::Folder(target) => {
                let Some(path) = self.checkout_folder() else {
                    return;
                };
                self.remember_folder_app(target.id, cx);
                self.close_nested(cx);
                cx.emit(CloudConversationEvent::OpenPathIn(target, path));
                self.close_computer(cx);
            }
            Pick::CopyPath => {
                self.close_nested(cx);
                if let Some(path) = self.checkout_folder() {
                    cx.write_to_clipboard(ClipboardItem::new_string(path));
                    crate::toast::show(cx, crate::toast::ToastKind::Success, "Path copied");
                }
            }
            Pick::DeleteFolder if !menu.confirm_delete => {
                if let Some(menu) = self.computer.nested.open_mut() {
                    menu.confirm_delete = true;
                    menu.highlighted = Some(index);
                }
                cx.notify();
            }
            Pick::DeleteFolder => {
                self.close_nested(cx);
                self.delete_checkout_folder(cx);
            }
            Pick::Ssh(action) => {
                self.close_nested(cx);
                if self.computer_running() {
                    self.open_ssh(action, cx);
                }
            }
        }
    }

    /// The menu of `kind` while open, anchored under its chevron. The caller
    /// adds it to the split button (which must be `relative`).
    pub(super) fn render_nested(
        &mut self,
        kind: Kind,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self
            .computer
            .nested
            .as_open()
            .filter(|menu| menu.kind == kind)?
            .clone();
        let items = self.nested_items(kind);
        let enabled = match kind {
            Kind::Folder => self.computer.checkout.change_idle(),
            Kind::Ssh => self.computer_running() && !self.ssh_busy(),
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if item.separated {
                rows.push(popover::menu_separator().into_any_element());
            }
            let highlighted = menu.highlighted == Some(index);
            let confirming = item.pick == Pick::DeleteFolder && menu.confirm_delete;
            let label = if confirming {
                "Delete folder and its files?"
            } else {
                item.label
            };
            let glyph = self.nested_glyph(&item.pick, theme);
            let tone = if item.danger {
                theme.danger
            } else {
                theme.text
            };
            let is_delete = item.pick == Pick::DeleteFolder;
            let id = item.id.clone();
            rows.push(
                popover::menu_row_nav(theme, false, highlighted, id.clone())
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .role(gpui::Role::MenuItem)
                    .aria_label(label)
                    .gap(px(10.))
                    .h(px(34.))
                    .text_size(crate::typography::text_sm())
                    .opacity(if enabled { 1. } else { 0.5 })
                    .when(item.danger, |row| row.text_color(tone))
                    .child(
                        div()
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .children(item.shortcut.map(|combo| {
                        popover::kbd_hint(theme, &crate::settings::badge_combo(combo))
                    }))
                    .children(item.number.map(|number| {
                        div()
                            .w(px(12.))
                            .flex()
                            .justify_end()
                            .child(popover::kbd_hint(theme, &number.to_string()))
                    }))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if let Some(menu) = this.computer.nested.open_mut()
                            && menu.highlighted != Some(index)
                        {
                            menu.highlighted = Some(index);
                            menu.confirm_delete &= is_delete;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if enabled {
                            this.pick_nested(index, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        let entity = cx.entity().downgrade();
        let card = popover::popover_card(theme)
            .id(match kind {
                Kind::Folder => "cloud-folder-menu",
                Kind::Ssh => "cloud-ssh-menu",
            })
            .w(px(248.))
            .font_family(crate::typography::effective_family_name(cx))
            .relative()
            .flex()
            .flex_col()
            .role(gpui::Role::Menu)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                // Stays mounted (closing) until the reap, so the chevron's
                // own press still reads it as open and leaves it closed.
                this.computer.nested_bounds = None;
                if this.computer.nested.begin_close() {
                    popover::reap_popup(cx, |this: &mut Self| &mut this.computer.nested);
                }
                cx.notify();
            }))
            .children(rows)
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = entity.update(cx, |this, _| {
                            if this.computer.nested.is_open() {
                                this.computer.nested_bounds = Some(bounds);
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        Some(popover::nested_menu_below_end(
            match kind {
                Kind::Folder => "cloud-folder-menu-layer",
                Kind::Ssh => "cloud-ssh-menu-layer",
            },
            card.into_any_element(),
        ))
    }

    fn nested_glyph(&self, pick: &Pick, theme: &Theme) -> AnyElement {
        use crate::shell::open_in::{app_icon, fallback_glyph};
        let glyph = |name: &'static str| {
            icon(name)
                .size(px(16.))
                .flex_none()
                .text_color(theme.text_muted)
                .into_any_element()
        };
        match pick {
            Pick::Folder(target) => crate::shell::open_in::target_icon(target, theme),
            Pick::CopyPath | Pick::Ssh(SshAction::Copy) => glyph(icons::COPY),
            Pick::DeleteFolder => icon(icons::TRASH_BIN_MINIMALISTIC)
                .size(px(16.))
                .flex_none()
                .text_color(theme.danger)
                .into_any_element(),
            Pick::Ssh(SshAction::Open(target)) => app_icon(
                target.app.id(),
                target.icon.as_ref(),
                if target.app.is_terminal() {
                    icons::TERMINAL
                } else {
                    fallback_glyph(target.app.id())
                },
                20.,
                theme,
            ),
            Pick::Ssh(_) => glyph(icons::TERMINAL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::open_in::fixture_targets;
    use crate::shell::ssh_open::SshApp;

    fn labels(items: &[Item]) -> Vec<(&'static str, Option<usize>)> {
        items.iter().map(|item| (item.label, item.number)).collect()
    }

    #[test]
    fn folder_menu_numbers_apps_then_copy_path_and_never_delete() {
        let targets = fixture_targets();
        let items = folder_items(&targets[..3], true);
        assert_eq!(
            labels(&items),
            [
                ("Files", Some(1)),
                ("Cursor", Some(2)),
                ("VS Code", Some(3)),
                ("Copy path", Some(4)),
                ("Delete folder", None),
            ]
        );
        assert_eq!(items[3].shortcut, Some(COPY_PATH_SHORTCUT));
        assert!(items[3].separated && items[4].separated && items[4].danger);
        // While syncing the folder can't be deleted from here.
        let items = folder_items(&targets[..1], false);
        assert_eq!(labels(&items), [("Files", Some(1)), ("Copy path", Some(2))]);
        // No apps detected yet: Copy path alone, with no hairline above it.
        assert!(!folder_items(&[], false)[0].separated);
    }

    #[test]
    fn numbers_stop_at_nine() {
        let targets: Vec<_> = (0..4).flat_map(|_| fixture_targets()).collect();
        let items = folder_items(&targets, false);
        assert_eq!(items[8].number, Some(9));
        assert!(items[9..].iter().all(|item| item.number.is_none()));
    }

    #[test]
    fn ssh_menu_lists_apps_then_copy_ssh_command() {
        let items = ssh_items(&[
            SshTarget::for_test(SshApp::Cursor),
            SshTarget::for_test(SshApp::Zed),
        ]);
        assert_eq!(
            labels(&items),
            [
                ("Cursor", Some(1)),
                ("Zed", Some(2)),
                ("Copy SSH command", Some(3)),
            ]
        );
        assert!(items[2].separated && !items[1].separated);
        assert_eq!(items[2].pick, Pick::Ssh(SshAction::Copy));
        // Before detection the system terminal stands in.
        assert_eq!(
            labels(&ssh_items(&[])),
            [("Terminal", Some(1)), ("Copy SSH command", Some(2))]
        );
    }

    #[test]
    fn keys_move_pick_and_close() {
        let items = folder_items(&fixture_targets()[..2], true);
        let plain = gpui::Modifiers::default();
        assert_eq!(
            key_outcome(&items, Some(0), "down", plain),
            KeyOutcome::Highlight(1)
        );
        assert_eq!(
            key_outcome(&items, Some(0), "up", plain),
            KeyOutcome::Highlight(3)
        );
        assert_eq!(
            key_outcome(&items, None, "down", plain),
            KeyOutcome::Highlight(0)
        );
        assert_eq!(
            key_outcome(&items, Some(1), "enter", plain),
            KeyOutcome::Pick(1)
        );
        assert_eq!(
            key_outcome(&items, None, "enter", plain),
            KeyOutcome::Ignore
        );
        assert_eq!(key_outcome(&items, None, "2", plain), KeyOutcome::Pick(1));
        assert_eq!(key_outcome(&items, None, "3", plain), KeyOutcome::Pick(2));
        // Delete folder has no number.
        assert_eq!(key_outcome(&items, None, "4", plain), KeyOutcome::Ignore);
        assert_eq!(
            key_outcome(&items, None, "escape", plain),
            KeyOutcome::Close
        );
        assert_eq!(key_outcome(&items, None, "x", plain), KeyOutcome::Ignore);
        let copy = gpui::Modifiers {
            shift: true,
            platform: cfg!(target_os = "macos"),
            control: !cfg!(target_os = "macos"),
            ..Default::default()
        };
        assert_eq!(key_outcome(&items, None, "c", copy), KeyOutcome::Pick(2));
        let command = gpui::Modifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(key_outcome(&items, None, "1", command), KeyOutcome::Ignore);
    }

    #[test]
    fn the_folder_button_opens_the_saved_app_while_installed() {
        let targets = fixture_targets();
        assert_eq!(default_target(&targets, Some("zed")).unwrap().id, "zed");
        assert_eq!(default_target(&targets, Some("gone")).unwrap().id, "finder");
        assert_eq!(default_target(&targets, None).unwrap().id, "finder");
        assert_eq!(default_target(&targets[1..2], None).unwrap().id, "cursor");
        assert!(default_target(&[], Some("zed")).is_none());
    }

    #[test]
    fn long_paths_lose_their_middle_folders() {
        let path = "~/zuse/cloud/opcodehq-zuse/8f3k2c1a";
        assert_eq!(middle_truncate(path, 60), path);
        assert_eq!(middle_truncate(path, 28), "~/…/opcodehq-zuse/8f3k2c1a");
        // Without the root one more folder fits.
        assert_eq!(
            middle_truncate("/Users/alex/zuse/cloud/opcodehq-zuse/8f3k2c1a", 32),
            "…/cloud/opcodehq-zuse/8f3k2c1a"
        );
        // A last folder too long for the space loses its own middle.
        let long = middle_truncate("~/a-very-long-folder-name-for-this-chat", 20);
        assert_eq!(long.chars().count(), 20);
        assert!(
            long.starts_with("~/a-v") && long.ends_with("this-chat"),
            "{long}"
        );
        assert_eq!(middle_truncate("abcdef", 2), "abcdef");
    }

    #[test]
    fn home_reads_as_a_tilde() {
        let home = Some("/Users/alex");
        assert_eq!(
            tilde("/Users/alex/zuse/cloud/web/8f3k2c1a", home),
            "~/zuse/cloud/web/8f3k2c1a"
        );
        assert_eq!(tilde("/Users/alex", Some("/Users/alex/")), "~");
        assert_eq!(tilde("/Users/alexa/code", home), "/Users/alexa/code");
        assert_eq!(tilde("/srv/web", None), "/srv/web");
    }
}
