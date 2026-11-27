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
