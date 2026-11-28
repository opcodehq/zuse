//! A Zapper's files as the tree the native and browser Drive show: folders
//! are the parents of live files plus the folders people created (see the
//! engine's `zappers/files.rs`), folders sort before files, and the reserved
//! root files always have a row so an older Zapper without one can write it.
use crate::zappers::{RESERVED_FILES, ZapperFile, ZapperFileVersion};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub folder: bool,
    pub expanded: bool,
    /// The file's current version; None for folders and for a reserved file
    /// not written yet.
    pub version: Option<u32>,
    pub reserved: bool,
    /// A file at a path that is also a folder, which only an older engine
    /// writes. Both rows show, so neither hides the other.
    pub conflict: bool,
}

impl FileRow {
    /// Identifies the row: a conflicting file and its folder share a path.
    pub fn key(&self) -> String {
        format!("{}:{}", if self.folder { "d" } else { "f" }, self.path)
    }
}

/// What a key does in the tree, given the selected row's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeKey {
    /// Select the row with this key.
    Select(String),
    Expand(String),
    Collapse(String),
    /// Open a file in its tab.
    Open(String),
    /// Rename or move, or delete, the selected row.
    Move(FileRow),
    Delete(FileRow),
}

/// The workspace explorer's keys: Up, Down, Home and End move; Right opens a
/// folder or steps into it; Left closes it or steps to its parent; Enter or
/// Space opens a file or toggles a folder; F2 moves and Delete or Backspace
/// deletes, except reserved files.
pub fn tree_key(rows: &[FileRow], selected: Option<&str>, key: &str) -> Option<TreeKey> {
    if rows.is_empty() {
        return None;
    }
    let at = selected.and_then(|s| rows.iter().position(|r| r.key() == s));
    let pick = |i: usize| Some(TreeKey::Select(rows[i.min(rows.len() - 1)].key()));
    let row = at.map(|i| &rows[i]);
    match key {
        "down" => pick(at.map_or(0, |i| i + 1)),
        "up" => pick(at.map_or(0, |i| i.saturating_sub(1))),
        "home" => pick(0),
        "end" => pick(rows.len() - 1),
        "right" => match row? {
            r if r.folder && !r.expanded => Some(TreeKey::Expand(r.path.clone())),
            r if r.folder => pick(at? + 1),
            _ => None,
        },
        "left" => match row? {
            r if r.folder && r.expanded => Some(TreeKey::Collapse(r.path.clone())),
            r => {
                let (parent, _) = r.path.rsplit_once('/')?;
                Some(TreeKey::Select(format!("d:{parent}")))
            }
        },
        "enter" | "space" => match row? {
            r if r.folder && r.expanded => Some(TreeKey::Collapse(r.path.clone())),
            r if r.folder => Some(TreeKey::Expand(r.path.clone())),
            r => Some(TreeKey::Open(r.path.clone())),
        },
        "f2" => row
            .filter(|r| !r.reserved)
            .map(|r| TreeKey::Move(r.clone())),
        "delete" | "backspace" => row
            .filter(|r| !r.reserved)
            .map(|r| TreeKey::Delete(r.clone())),
        _ => None,
    }
}

/// A reserved root file's role, shown beside its name so a look-alike name
/// can't pass for it. Matching is exact.
pub fn reserved_role(path: &str) -> Option<&'static str> {
    Some(match path {
        "AGENTS.md" => "Coordinator rules",
        "WORKERS.md" => "Thread rules",
        "NOTES.md" => "Notes",
        "INSTRUCTIONS.md" => "Purpose",
        "DIGEST.md" => "Digest",
        _ => return None,
    })
}

/// Each live file's newest version; `files` may be a whole history.
pub fn latest(files: &[ZapperFile]) -> Vec<&ZapperFile> {
    let mut newest: BTreeMap<&str, &ZapperFile> = BTreeMap::new();
    for file in files {
        if newest
            .get(file.path.as_str())
            .is_none_or(|f| f.version < file.version)
        {
            newest.insert(&file.path, file);
        }
    }
    newest.into_values().filter(|f| !f.deleted).collect()
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Every folder above `path`, outermost first.
pub fn ancestors(path: &str) -> Vec<String> {
    path.match_indices('/')
        .map(|(end, _)| path[..end].to_owned())
        .collect()
}

fn all_folders(files: &[&ZapperFile], folders: &[String]) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for path in folders.iter().chain(files.iter().map(|f| &f.path)) {
        all.extend(ancestors(path));
    }
    all.extend(folders.iter().cloned());
    all
}

/// The rows shown with `expanded` folders open.
pub fn tree_rows(
    files: &[ZapperFile],
    folders: &[String],
    expanded: &BTreeSet<String>,
) -> Vec<FileRow> {
    let live = latest(files);
    let all = all_folders(&live, folders);
    let mut children: BTreeMap<&str, Vec<FileRow>> = BTreeMap::new();
    let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    for folder in &all {
        children.entry(parent(folder)).or_default().push(FileRow {
            path: folder.clone(),
            name: name(folder),
            depth: 0,
            folder: true,
            expanded: expanded.contains(folder),
            version: None,
            reserved: false,
            conflict: false,
        });
    }
    for file in &live {
        children
            .entry(parent(&file.path))
            .or_default()
            .push(FileRow {
                path: file.path.clone(),
                name: name(&file.path),
                depth: 0,
                folder: false,
                expanded: false,
                version: Some(file.version),
                reserved: RESERVED_FILES.contains(&file.path.as_str()),
                conflict: all.contains(&file.path),
            });
    }
    for path in RESERVED_FILES {
        if !live.iter().any(|f| f.path == path) {
            children.entry("").or_default().push(FileRow {
                path: path.to_owned(),
                name: path.to_owned(),
                depth: 0,
                folder: false,
                expanded: false,
                version: None,
                reserved: true,
                conflict: false,
            });
        }
    }
    for rows in children.values_mut() {
        rows.sort_by(|a, b| {
            b.folder
                .cmp(&a.folder)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    let mut out = Vec::new();
    fn walk(
        at: &str,
        depth: usize,
        children: &BTreeMap<&str, Vec<FileRow>>,
        out: &mut Vec<FileRow>,
    ) {
        for row in children.get(at).into_iter().flatten() {
            out.push(FileRow {
                depth,
                ..row.clone()
            });
            if row.folder && row.expanded {
                walk(&row.path, depth + 1, children, out);
            }
        }
    }
    walk("", 0, &children, &mut out);
    out
}

/// Top-level folders holding something, opened the first time Drive shows.
pub fn initially_open(files: &[ZapperFile], folders: &[String]) -> BTreeSet<String> {
    let live = latest(files);
    all_folders(&live, folders)
        .into_iter()
        .filter(|f| !f.contains('/'))
        .filter(|f| {
            let inside = format!("{f}/");
            live.iter().any(|file| file.path.starts_with(&inside))
                || folders.iter().any(|g| g.starts_with(&inside))
        })
        .collect()
}

/// Every live file under `folder` at its newest version: what a folder move
/// or delete names so the engine can refuse it if anything changed.
pub fn files_under(files: &[ZapperFile], folder: &str) -> Vec<ZapperFileVersion> {
    let inside = format!("{folder}/");
    latest(files)
        .into_iter()
        .filter(|f| f.path.starts_with(&inside))
        .map(|f| ZapperFileVersion {
            path: f.path.clone(),
            version: f.version,
        })
        .collect()
}

/// Created folders strictly under `folder`.
pub fn folders_under(folders: &[String], folder: &str) -> Vec<String> {
    let inside = format!("{folder}/");
    folders
        .iter()
        .filter(|f| f.starts_with(&inside))
        .cloned()
        .collect()
}

/// What a reserved root file is for, as Drive explains it.
pub fn reserved_about(path: &str) -> Option<&'static str> {
    Some(match path {
        "AGENTS.md" => {
            "Your standing rules for the coordinator. It follows them from its next turn; only people edit this file."
        }
        "WORKERS.md" => "Rules every thread gets with its prompt, from its next turn.",
        "NOTES.md" => {
            "Notes the coordinator keeps between turns. You can edit them; it reads them as context, never as instructions."
        }
        "INSTRUCTIONS.md" => "The coordinator's purpose and working rules, in its own words.",
        "DIGEST.md" => "The coordinator's short status summary for you.",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, version: u32, deleted: bool) -> ZapperFile {
        ZapperFile {
            path: path.into(),
            version,
            content: String::new(),
            deleted,
        }
    }

    #[test]
    fn folders_sort_first_nest_and_keep_created_empty_ones() {
        let files = [
            file("handoffs/research.md", 1, false),
            file("handoffs/design/a.md", 1, false),
            file("NOTES.md", 1, false),
            file("gone.md", 1, false),
            file("gone.md", 2, true),
        ];
        let folders = vec!["reports".to_owned(), "handoffs/empty".to_owned()];
        let open = initially_open(&files, &folders);
        assert_eq!(open, BTreeSet::from(["handoffs".to_owned()]));
        let rows: Vec<String> = tree_rows(&files, &folders, &open)
            .iter()
            .map(|r| {
                format!(
                    "{}{}{}",
                    "  ".repeat(r.depth),
                    r.name,
                    if r.version.is_none() && !r.folder {
                        " (not written)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                "handoffs",
                "  design",
                "  empty",
                "  research.md",
                "reports",
                "AGENTS.md (not written)",
                "DIGEST.md (not written)",
                "INSTRUCTIONS.md (not written)",
                "NOTES.md",
                "WORKERS.md (not written)",
            ]
        );
    }

    #[test]
    fn a_file_an_older_engine_wrote_over_a_folder_shows_beside_it() {
        let files = [
            file("notes/day.md", 1, false),
            file("notes", 1, false),
            file("handoffs", 1, false),
        ];
        let folders = vec!["handoffs".to_owned()];
        let open = BTreeSet::from(["notes".to_owned()]);
        let rows: Vec<(String, bool)> = tree_rows(&files, &folders, &open)
            .iter()
            .filter(|r| !r.reserved)
            .map(|r| (format!("{}{}", "  ".repeat(r.depth), r.key()), r.conflict))
            .collect();
        assert_eq!(
            rows,
            [
                ("d:handoffs".to_owned(), false),
                ("d:notes".into(), false),
                ("  f:notes/day.md".into(), false),
                ("f:handoffs".into(), true),
                ("f:notes".into(), true),
            ]
        );
    }

    #[test]
    fn keys_move_through_the_tree_and_never_move_or_delete_reserved_files() {
        let files = [
            file("a/x.md", 1, false),
            file("a/y.md", 1, false),
            file("NOTES.md", 1, false),
        ];
        let open = BTreeSet::from(["a".to_owned()]);
        let rows = tree_rows(&files, &[], &open);
        let key = |selected: &str, key: &str| tree_key(&rows, Some(selected), key);
        assert_eq!(
            key("f:a/x.md", "down"),
            Some(TreeKey::Select("f:a/y.md".into()))
        );
        assert_eq!(key("d:a", "up"), Some(TreeKey::Select("d:a".into())));
        assert_eq!(key("f:a/y.md", "left"), Some(TreeKey::Select("d:a".into())));
        assert_eq!(key("d:a", "left"), Some(TreeKey::Collapse("a".into())));
        assert_eq!(
            key("d:a", "right"),
            Some(TreeKey::Select("f:a/x.md".into()))
        );
        assert_eq!(
            key("f:a/x.md", "enter"),
            Some(TreeKey::Open("a/x.md".into()))
        );
        assert!(matches!(key("f:a/x.md", "f2"), Some(TreeKey::Move(r)) if r.path == "a/x.md"));
        assert!(matches!(key("d:a", "delete"), Some(TreeKey::Delete(r)) if r.folder));
        assert_eq!(key("f:NOTES.md", "f2"), None);
        assert_eq!(key("f:NOTES.md", "delete"), None);
        assert_eq!(
            tree_key(&rows, None, "down"),
            Some(TreeKey::Select("d:a".into()))
        );
        let closed = tree_rows(&files, &[], &BTreeSet::new());
        assert_eq!(
            tree_key(&closed, Some("d:a"), "enter"),
            Some(TreeKey::Expand("a".into()))
        );
    }

    #[test]
    fn folder_actions_name_every_live_file_under_it_and_nothing_beside_it() {
        let files = [
            file("a/x.md", 3, false),
            file("a/x.md", 4, false),
            file("a/deep/y.md", 1, false),
            file("ab.md", 1, false),
            file("a/old.md", 1, false),
            file("a/old.md", 2, true),
        ];
        assert_eq!(
            files_under(&files, "a"),
            [
                ZapperFileVersion {
                    path: "a/deep/y.md".into(),
                    version: 1
                },
                ZapperFileVersion {
                    path: "a/x.md".into(),
                    version: 4
                },
            ]
        );
        assert_eq!(
            folders_under(&["a".into(), "a/b".into(), "ab".into()], "a"),
            ["a/b"]
        );
    }
}
