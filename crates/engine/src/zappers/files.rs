//! A Zapper's files as a tree. Files are versioned rows in `zapper_files`;
//! folders are the parent paths of live files plus the folders a person
//! created, kept in `zapper_folders` so an empty one stays put. Deleting a
//! file writes a new, empty version and records it in
//! `zapper_file_deletions`, so history and restore work across deletes and an
//! older engine reading the same database sees an empty file instead of
//! failing. Folder moves and deletes name every file they touch at the
//! version the caller saw, and run in the command's transaction, so a file
//! added or edited meanwhile, or one a client didn't show, is never swept up.
use super::*;
use std::collections::BTreeMap;

/// The user's standing rules for the coordinator. Only people edit it.
pub(crate) const RULES_FILE: &str = "AGENTS.md";
/// Rules prepended to every thread's prompt.
pub(crate) const WORKERS_FILE: &str = "WORKERS.md";
/// Notes the coordinator keeps and the user can edit; context, not rules.
pub(crate) const NOTES_FILE: &str = "NOTES.md";
pub(crate) const DIGEST_FILE: &str = "DIGEST.md";

/// Root files with a fixed meaning: they can be edited and restored, never
/// deleted, moved, or replaced by a folder.
pub(crate) const RESERVED: [&str; 5] = [
    PURPOSE_FILE,
    RULES_FILE,
    WORKERS_FILE,
    NOTES_FILE,
    DIGEST_FILE,
];

/// Folders a new Zapper starts with. Files are text, so there is no folder
/// for images.
pub(super) const DEFAULT_FOLDERS: [&str; 2] = ["handoffs", "reports"];

const PATH_MAX: usize = 256;

pub(super) fn create_tables(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS zapper_folders(zapper TEXT NOT NULL, path TEXT NOT NULL,
            PRIMARY KEY(zapper,path));
        CREATE TABLE IF NOT EXISTS zapper_file_deletions(zapper TEXT NOT NULL, path TEXT NOT NULL,
            version INTEGER NOT NULL, PRIMARY KEY(zapper,path,version));",
    )?;
    Ok(())
}

/// Whether `path` may be stored at all: relative, `/`-separated, without
/// empty, `.` or `..` segments or backslashes. Paths saved before stricter
/// rules existed keep passing this, so they stay editable.
pub(super) fn check_stored(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= PATH_MAX
            && !path.starts_with('/')
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".." && !p.contains('\\')),
        "invalid Zapper file path"
    );
    Ok(())
}

/// Whether a new file or folder may take `path`: the stored rules, plus no
/// control characters and no spaces around a name, and not a guide.
fn check_new(path: &str) -> Result<()> {
    check_stored(path)?;
    ensure!(
        path.split('/')
            .all(|p| p.trim() == p && !p.chars().any(char::is_control)),
        "invalid Zapper file path {path:?}: names can't start or end with spaces or hold control characters"
    );
    ensure!(
        !path.starts_with(guide::GUIDE_DIR) && path != guide::GUIDE_DIR.trim_end_matches('/'),
        "{path} is a read-only guide"
    );
    Ok(())
}

fn check_not_reserved(path: &str) -> Result<()> {
    ensure!(
        !RESERVED.contains(&path),
        "{path} is one of the Zapper's own files: edit it, but it can't be moved, deleted or replaced"
    );
    Ok(())
}

/// Each live file's newest version; deleted files are left out.
pub(super) fn live(db: &Connection, zapper: &str) -> Result<BTreeMap<String, u32>> {
    let mut stmt = db.prepare_cached(
        "SELECT f.path,f.version FROM zapper_files f WHERE f.zapper=?1
           AND f.version=(SELECT MAX(version) FROM zapper_files WHERE zapper=f.zapper AND path=f.path)
           AND NOT EXISTS(SELECT 1 FROM zapper_file_deletions d
               WHERE d.zapper=f.zapper AND d.path=f.path AND d.version=f.version)",
    )?;
    Ok(stmt
        .query_map([zapper], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Folders people created, sorted.
pub(super) fn folders(db: &Connection, zapper: &str) -> Result<Vec<String>> {
    let mut stmt =
        db.prepare_cached("SELECT path FROM zapper_folders WHERE zapper=?1 ORDER BY path")?;
    Ok(stmt
        .query_map([zapper], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// The newest version of `path`, deleted or not, and whether it is deleted.
fn newest(db: &Connection, zapper: &str, path: &str) -> Result<(u32, bool)> {
    let version: u32 = db.query_row(
        "SELECT COALESCE(MAX(version),0) FROM zapper_files WHERE zapper=?1 AND path=?2",
        params![zapper, path],
        |r| r.get(0),
    )?;
    Ok((version, is_deletion(db, zapper, path, version)?))
}

pub(super) fn is_deletion(db: &Connection, zapper: &str, path: &str, version: u32) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM zapper_file_deletions WHERE zapper=?1 AND path=?2 AND version=?3",
            params![zapper, path, version],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn is_folder(db: &Connection, zapper: &str, path: &str) -> Result<bool> {
    let inside = format!("{path}/");
    Ok(folders(db, zapper)?
        .iter()
        .any(|f| f == path || f.starts_with(&inside))
        || live(db, zapper)?.keys().any(|f| f.starts_with(&inside)))
}

/// Refuses `path` for a new file or folder when a file or folder is already
/// there, or when one of its parents is a file.
fn check_free(db: &Connection, zapper: &str, path: &str) -> Result<()> {
    let files = live(db, zapper)?;
    ensure!(!files.contains_key(path), "{path} already exists");
    ensure!(
        !is_folder(db, zapper, path)?,
        "a folder named {path} already exists"
    );
    for (end, _) in path.match_indices('/') {
        let parent = &path[..end];
        ensure!(
            !files.contains_key(parent),
            "{parent} is a file, so it can't hold {path}"
        );
    }
    Ok(())
}

/// Checks a write before it lands: a path without a live file is a new file
/// and must be free. Returns the version the write replaces.
pub(super) fn check_write(tx: &rusqlite::Transaction<'_>, zapper: &str, path: &str) -> Result<u32> {
    check_stored(path)?;
    ensure!(
        !path.starts_with(guide::GUIDE_DIR),
        "{path} is a read-only guide"
    );
    let (version, deleted) = newest(tx, zapper, path)?;
    if version == 0 || deleted {
        if !RESERVED.contains(&path) {
            check_new(path)?;
        }
        check_free(tx, zapper, path)?;
    }
    Ok(version)
}

pub(super) fn conflict(path: &str, expected: u32, version: u32) -> anyhow::Error {
    anyhow::anyhow!(
        "Zapper file version conflict on {path}: expected {expected}, current version is {version}. Re-read it with zapper_files and merge before writing"
    )
}

pub(super) fn insert(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    path: &str,
    version: u32,
    content: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO zapper_files VALUES(?1,?2,?3,?4)",
        params![zapper, path, version, content],
    )?;
    Ok(())
}

/// Deletes a live file as a new, empty version. Returns its last content.
pub(super) fn delete_file(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    path: &str,
    expected: u32,
) -> Result<String> {
    check_not_reserved(path)?;
    let (version, deleted) = newest(tx, zapper, path)?;
    ensure!(version > 0 && !deleted, "no Zapper file at {path}");
    if version != expected {
        return Err(conflict(path, expected, version));
    }
    let content: String = tx.query_row(
        "SELECT content FROM zapper_files WHERE zapper=?1 AND path=?2 AND version=?3",
        params![zapper, path, version],
        |r| r.get(0),
    )?;
    insert(tx, zapper, path, version + 1, "")?;
    tx.execute(
        "INSERT INTO zapper_file_deletions VALUES(?1,?2,?3)",
        params![zapper, path, version + 1],
    )?;
    Ok(content)
}

/// Writes `content` as a new file at `to`, continuing any history a deleted
/// file there left.
fn create_at(tx: &rusqlite::Transaction<'_>, zapper: &str, to: &str, content: &str) -> Result<()> {
    check_free(tx, zapper, to)?;
    let version = check_write(tx, zapper, to)?;
    insert(tx, zapper, to, version + 1, content)
}

pub(super) fn move_file(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    path: &str,
    to: &str,
    expected: u32,
) -> Result<()> {
    ensure!(path != to, "{path} is already there");
    check_not_reserved(to)?;
    check_new(to)?;
    let content = delete_file(tx, zapper, path, expected)?;
    create_at(tx, zapper, to, &content)
}

pub(super) fn create_folder(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    path: &str,
) -> Result<()> {
    check_new(path)?;
    check_not_reserved(path)?;
    check_free(tx, zapper, path)?;
    tx.execute(
        "INSERT INTO zapper_folders VALUES(?1,?2)",
        params![zapper, path],
    )?;
    Ok(())
}

/// The files under `folder`, refusing unless they are exactly `seen`.
fn snapshot(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    folder: &str,
    seen: &[ZapperFileVersion],
) -> Result<BTreeMap<String, u32>> {
    check_stored(folder)?;
    ensure!(is_folder(tx, zapper, folder)?, "no folder at {folder}");
    let inside = format!("{folder}/");
    let current: BTreeMap<String, u32> = live(tx, zapper)?
        .into_iter()
        .filter(|(path, _)| path.starts_with(&inside))
        .collect();
    let seen: BTreeMap<String, u32> = seen.iter().map(|f| (f.path.clone(), f.version)).collect();
    ensure!(
        current == seen,
        "{folder} changed since it was read: it now holds {} files and the request named {}. Re-read it with zapper_files and confirm again",
        current.len(),
        seen.len()
    );
    Ok(current)
}

/// Created folders at or under `folder`.
fn folder_rows(tx: &rusqlite::Transaction<'_>, zapper: &str, folder: &str) -> Result<Vec<String>> {
    let inside = format!("{folder}/");
    Ok(folders(tx, zapper)?
        .into_iter()
        .filter(|f| f == folder || f.starts_with(&inside))
        .collect())
}

pub(super) fn move_folder(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    folder: &str,
    to: &str,
    seen: &[ZapperFileVersion],
) -> Result<()> {
    ensure!(
        to != folder && !to.starts_with(&format!("{folder}/")),
        "a folder can't move into itself"
    );
    let files = snapshot(tx, zapper, folder, seen)?;
    check_new(to)?;
    check_not_reserved(to)?;
    check_free(tx, zapper, to)?;
    let rows = folder_rows(tx, zapper, folder)?;
    for (path, version) in &files {
        let content = delete_file(tx, zapper, path, *version)?;
        create_at(
            tx,
            zapper,
            &format!("{to}{}", &path[folder.len()..]),
            &content,
        )?;
    }
    for row in rows {
        tx.execute(
            "DELETE FROM zapper_folders WHERE zapper=?1 AND path=?2",
            params![zapper, row],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO zapper_folders VALUES(?1,?2)",
            params![zapper, format!("{to}{}", &row[folder.len()..])],
        )?;
    }
    Ok(())
}

pub(super) fn delete_folder(
    tx: &rusqlite::Transaction<'_>,
    zapper: &str,
    folder: &str,
    seen: &[ZapperFileVersion],
) -> Result<()> {
    let files = snapshot(tx, zapper, folder, seen)?;
    for (path, version) in &files {
        delete_file(tx, zapper, path, *version)?;
    }
    for row in folder_rows(tx, zapper, folder)? {
        tx.execute(
            "DELETE FROM zapper_folders WHERE zapper=?1 AND path=?2",
            params![zapper, row],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zappers::tests::{command, create};

    fn store() -> (tempfile::TempDir, Zappers) {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        (dir, store)
    }

    fn run(store: &Zappers, action: ZapperAction) -> Result<Option<Zapper>> {
        store.command(command(
            "release",
            &uuid::Uuid::new_v4().simple().to_string(),
            action,
        ))
    }

    fn write(
        store: &Zappers,
        path: &str,
        content: &str,
        expected_version: u32,
    ) -> Result<Option<Zapper>> {
        run(
            store,
            ZapperAction::WriteFile {
                path: path.into(),
                content: content.into(),
                expected_version,
            },
        )
    }

    fn live_paths(store: &Zappers) -> Vec<(String, u32, String)> {
        store
            .files("release", false)
            .unwrap()
            .into_iter()
            .map(|f| (f.path, f.version, f.content))
            .collect()
    }

    fn seen(store: &Zappers, folder: &str) -> Vec<ZapperFileVersion> {
        store
            .files("release", false)
            .unwrap()
            .into_iter()
            .filter(|f| f.path.starts_with(&format!("{folder}/")))
            .map(|f| ZapperFileVersion {
                path: f.path,
                version: f.version,
            })
            .collect()
    }

    #[test]
    fn new_zappers_get_their_files_and_default_folders() {
        let (_dir, store) = store();
        let paths: Vec<_> = live_paths(&store).into_iter().map(|f| f.0).collect();
        assert_eq!(
            paths,
            [
                "AGENTS.md",
                "DIGEST.md",
                "INSTRUCTIONS.md",
                "NOTES.md",
                "WORKERS.md"
            ]
        );
        assert_eq!(
            store.get("release").unwrap().folders,
            ["handoffs", "reports"]
        );
    }

    #[test]
    fn existing_zappers_and_customized_files_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        create(&store, "release");
        // A Zapper saved before AGENTS.md and folders existed.
        {
            let db = store.inner.db.lock().unwrap();
            db.execute("DELETE FROM zapper_files WHERE path='AGENTS.md'", [])
                .unwrap();
            db.execute("DELETE FROM zapper_folders", []).unwrap();
        }
        let rules = "# Thread rules\n\nRun the full suite.\n";
        write(&store, "WORKERS.md", rules, 1).unwrap();
        drop(store);
        let store = Zappers::open(dir.path(), "local").unwrap();
        assert!(store.get("release").unwrap().folders.is_empty());
        let files = live_paths(&store);
        assert!(!files.iter().any(|f| f.0 == "AGENTS.md"));
        assert_eq!(
            files.iter().find(|f| f.0 == "WORKERS.md").unwrap(),
            &("WORKERS.md".to_owned(), 2, rules.to_owned())
        );
        write(&store, "AGENTS.md", "Ask before deploys.", 0).unwrap();
        assert!(
            live_paths(&store)
                .iter()
                .any(|f| f.0 == "AGENTS.md" && f.1 == 1)
        );
    }

    #[test]
    fn hostile_paths_and_collisions_are_refused() {
        let (_dir, store) = store();
        for path in [
            "../escape.md",
            "/etc/passwd",
            "a//b.md",
            "a\\b.md",
            "./a.md",
            "a/../b.md",
            " lead.md",
            "trail.md ",
            "a/ b.md",
            "bell\u{7}.md",
            "guide/commands.md",
            &"x".repeat(257),
        ] {
            assert!(write(&store, path, "x", 0).is_err(), "wrote {path:?}");
            assert!(
                run(&store, ZapperAction::CreateFolder { path: path.into() }).is_err(),
                "created folder {path:?}"
            );
        }
        for folder in ["guide", "AGENTS.md", "NOTES.md", "handoffs"] {
            assert!(
                run(
                    &store,
                    ZapperAction::CreateFolder {
                        path: folder.into()
                    }
                )
                .is_err(),
                "created folder {folder}"
            );
        }
        let err = write(&store, "handoffs", "x", 0).unwrap_err().to_string();
        assert!(
            err.contains("a folder named handoffs already exists"),
            "{err}"
        );
        write(&store, "plan.md", "x", 0).unwrap();
        let err = write(&store, "plan.md/inner.md", "x", 0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan.md is a file"), "{err}");
        write(&store, "notes/day.md", "x", 0).unwrap();
        assert!(write(&store, "notes", "x", 0).is_err());
        assert!(
            run(
                &store,
                ZapperAction::CreateFolder {
                    path: "notes".into()
                }
            )
            .is_err()
        );
        assert!(
            run(
                &store,
                ZapperAction::CreateFolder {
                    path: "plan.md/sub".into()
                }
            )
            .is_err()
        );
        run(
            &store,
            ZapperAction::CreateFolder {
                path: "handoffs/design".into(),
            },
        )
        .unwrap();
        assert!(write(&store, "handoffs/design", "x", 0).is_err());
    }

    #[test]
    fn deleting_writes_a_restorable_version() {
        let (_dir, store) = store();
        write(&store, "reports/q3.md", "numbers", 0).unwrap();
        let err = run(
            &store,
            ZapperAction::DeleteFile {
                path: "reports/q3.md".into(),
                expected_version: 0,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("version conflict"), "{err}");
        run(
            &store,
            ZapperAction::DeleteFile {
                path: "reports/q3.md".into(),
                expected_version: 1,
            },
        )
        .unwrap();
        assert!(!live_paths(&store).iter().any(|f| f.0 == "reports/q3.md"));
        assert!(store.file("release", "reports/q3.md").is_err());
        let history: Vec<_> = store
            .files("release", true)
            .unwrap()
            .into_iter()
            .filter(|f| f.path == "reports/q3.md")
            .map(|f| (f.version, f.deleted, f.content))
            .collect();
        assert_eq!(
            history,
            [(2, true, String::new()), (1, false, "numbers".into())]
        );
        assert!(
            run(
                &store,
                ZapperAction::RestoreFile {
                    path: "reports/q3.md".into(),
                    version: 2,
                    expected_version: 2,
                },
            )
            .is_err(),
            "a deletion is not a version to restore"
        );
        assert!(
            run(
                &store,
                ZapperAction::RestoreFile {
                    path: "reports/q3.md".into(),
                    version: 1,
                    expected_version: 1,
                },
            )
            .is_err(),
            "restoring over a deletion needs its version"
        );
        run(
            &store,
            ZapperAction::RestoreFile {
                path: "reports/q3.md".into(),
                version: 1,
                expected_version: 2,
            },
        )
        .unwrap();
        assert!(live_paths(&store).contains(&("reports/q3.md".into(), 3, "numbers".into())));
        for reserved in RESERVED {
            let version = store.file("release", reserved).unwrap().version;
            assert!(
                run(
                    &store,
                    ZapperAction::DeleteFile {
                        path: reserved.into(),
                        expected_version: version,
                    },
                )
                .is_err(),
                "deleted {reserved}"
            );
        }
    }

    #[test]
    fn restoring_into_a_path_now_taken_by_a_folder_is_refused() {
        let (_dir, store) = store();
        write(&store, "draft", "words", 0).unwrap();
        run(
            &store,
            ZapperAction::DeleteFile {
                path: "draft".into(),
                expected_version: 1,
            },
        )
        .unwrap();
        run(
            &store,
            ZapperAction::CreateFolder {
                path: "draft".into(),
            },
        )
        .unwrap();
        let err = run(
            &store,
            ZapperAction::RestoreFile {
                path: "draft".into(),
                version: 1,
                expected_version: 2,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("a folder named draft already exists"), "{err}");
    }

    #[test]
    fn moving_a_file_keeps_its_content_and_refuses_taken_or_reserved_paths() {
        let (_dir, store) = store();
        write(&store, "plan.md", "the plan", 0).unwrap();
        write(&store, "other.md", "other", 0).unwrap();
        let mv = |path: &str, to: &str, expected_version| {
            run(
                &store,
                ZapperAction::MoveFile {
                    path: path.into(),
                    to: to.into(),
                    expected_version,
                },
            )
        };
        assert!(mv("plan.md", "other.md", 1).is_err());
        assert!(mv("plan.md", "AGENTS.md", 1).is_err());
        assert!(mv("plan.md", "../plan.md", 1).is_err());
        assert!(mv("NOTES.md", "notes.md", 1).is_err());
        assert!(mv("plan.md", "handoffs/plan.md", 7).is_err());
        assert!(live_paths(&store).contains(&("plan.md".into(), 1, "the plan".into())));
        mv("plan.md", "handoffs/plan.md", 1).unwrap();
        let files = live_paths(&store);
        assert!(!files.iter().any(|f| f.0 == "plan.md"));
        assert!(files.contains(&("handoffs/plan.md".into(), 1, "the plan".into())));
    }

    #[test]
    fn folder_moves_and_deletes_act_only_on_the_exact_snapshot() {
        let (_dir, store) = store();
        write(&store, "handoffs/a.md", "a", 0).unwrap();
        write(&store, "handoffs/deep/b.md", "b", 0).unwrap();
        run(
            &store,
            ZapperAction::CreateFolder {
                path: "handoffs/empty".into(),
            },
        )
        .unwrap();
        let before = live_paths(&store);

        let partial = vec![ZapperFileVersion {
            path: "handoffs/a.md".into(),
            version: 1,
        }];
        let err = run(
            &store,
            ZapperAction::DeleteFolder {
                path: "handoffs".into(),
                files: partial.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("changed since it was read"), "{err}");
        assert_eq!(
            live_paths(&store),
            before,
            "nothing is deleted on a refusal"
        );

        let snapshot = seen(&store, "handoffs");
        write(&store, "handoffs/late.md", "late", 0).unwrap();
        assert!(
            run(
                &store,
                ZapperAction::MoveFolder {
                    path: "handoffs".into(),
                    to: "archive/handoffs".into(),
                    files: snapshot,
                },
            )
            .is_err(),
            "a file added after the read is never swept along"
        );

        for to in [
            "handoffs/inner",
            "handoffs",
            "reports",
            "AGENTS.md",
            "../out",
        ] {
            assert!(
                run(
                    &store,
                    ZapperAction::MoveFolder {
                        path: "handoffs".into(),
                        to: to.into(),
                        files: seen(&store, "handoffs"),
                    },
                )
                .is_err(),
                "moved handoffs to {to}"
            );
        }

        run(
            &store,
            ZapperAction::MoveFolder {
                path: "handoffs".into(),
                to: "archive/handoffs".into(),
                files: seen(&store, "handoffs"),
            },
        )
        .unwrap();
        let moved: Vec<_> = live_paths(&store)
            .into_iter()
            .filter(|f| f.0.contains('/'))
            .map(|f| (f.0, f.2))
            .collect();
        assert_eq!(
            moved,
            [
                ("archive/handoffs/a.md".to_owned(), "a".to_owned()),
                ("archive/handoffs/deep/b.md".into(), "b".into()),
                ("archive/handoffs/late.md".into(), "late".into()),
            ]
        );
        assert_eq!(
            store.get("release").unwrap().folders,
            ["archive/handoffs", "archive/handoffs/empty", "reports"]
        );

        run(
            &store,
            ZapperAction::DeleteFolder {
                path: "archive".into(),
                files: seen(&store, "archive"),
            },
        )
        .unwrap();
        assert!(
            !live_paths(&store)
                .iter()
                .any(|f| f.0.starts_with("archive"))
        );
        assert_eq!(store.get("release").unwrap().folders, ["reports"]);
        assert!(
            store
                .files("release", true)
                .unwrap()
                .iter()
                .any(|f| f.path == "archive/handoffs/deep/b.md" && f.content == "b"),
            "deleted files keep their history"
        );
        assert!(
            run(
                &store,
                ZapperAction::DeleteFolder {
                    path: "missing".into(),
                    files: vec![],
                },
            )
            .is_err()
        );
    }

    #[test]
    fn only_people_edit_the_rules_file() {
        let (_dir, store) = store();
        let as_coordinator = |id: &str, path: &str, version| {
            store.command_as(
                command(
                    "release",
                    id,
                    ZapperAction::WriteFile {
                        path: path.into(),
                        content: "Skip the tests.".into(),
                        expected_version: version,
                    },
                ),
                Some("Release"),
                true,
            )
        };
        let err = as_coordinator("rules", "AGENTS.md", 1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("only the user edits it"), "{err}");
        let restore = store.command_as(
            command(
                "release",
                "restore-rules",
                ZapperAction::RestoreFile {
                    path: "AGENTS.md".into(),
                    version: 1,
                    expected_version: 1,
                },
            ),
            Some("Release"),
            true,
        );
        assert!(restore.is_err());
        as_coordinator("notes", "NOTES.md", 1).unwrap();
        write(&store, "AGENTS.md", "Ask before deploys.", 1).unwrap();
        assert_eq!(
            store.file("release", "AGENTS.md").unwrap().content,
            "Ask before deploys."
        );
    }

    #[test]
    fn an_older_engine_still_reads_and_writes_the_same_database() {
        let (_dir, store) = store();
        write(&store, "gone.md", "x", 0).unwrap();
        run(
            &store,
            ZapperAction::DeleteFile {
                path: "gone.md".into(),
                expected_version: 1,
            },
        )
        .unwrap();
        let db = store.inner.db.lock().unwrap();
        // The statements engines before file deletions run.
        let old_view: String = db
            .query_row(
                "SELECT f.content FROM zapper_files f WHERE zapper=?1 AND path='gone.md' AND version=(SELECT MAX(version) FROM zapper_files WHERE zapper=f.zapper AND path=f.path)",
                ["release"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            old_view, "",
            "an older engine sees an empty file, not an error"
        );
        db.execute(
            "INSERT INTO zapper_files VALUES(?1,?2,?3,?4)",
            params!["release", "gone.md", 3, "written by an older engine"],
        )
        .unwrap();
        drop(db);
        assert!(live_paths(&store).contains(&(
            "gone.md".into(),
            3,
            "written by an older engine".into()
        )));
    }
}
