//! One-time move of Zapper state from its names before Zots were renamed
//! Zappers: the `zots` profile folder and the `zot_*` tables with their `zot`
//! columns. Every step checks before it acts, so it runs on each open and
//! resumes cleanly after a crash. Legacy, remove after one release.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// Tables before and after the rename.
const TABLES: &[(&str, &str)] = &[
    ("zots", "zappers"),
    ("zot_receipts", "zapper_receipts"),
    ("zot_workers", "zapper_workers"),
    ("zot_files", "zapper_files"),
    ("zot_events", "zapper_events"),
    ("zot_outbox", "zapper_outbox"),
    ("zot_delivery", "zapper_delivery"),
    ("zot_notifications", "zapper_notifications"),
    ("zot_turn_summaries", "zapper_turn_summaries"),
    ("zot_worker_context", "zapper_worker_context"),
    ("zot_binding_versions", "zapper_binding_versions"),
    ("zot_releases", "zapper_releases"),
    ("zot_worker_holds", "zapper_worker_holds"),
    ("zot_targets", "zapper_targets"),
    ("zot_coordinator_turns", "zapper_coordinator_turns"),
    ("zot_queue", "zapper_queue"),
];
const LEGACY_COLUMN: &str = "zot";

/// Copy the state database and control token from `legacy` into `root`.
/// Per-Zapper folders stay where they are (see `Zappers::workspace`).
pub(super) fn move_state(legacy: &Path, root: &Path) -> Result<()> {
    let token = root.join("control-token");
    let old_token = legacy.join("control-token");
    if !token.exists() && old_token.exists() {
        std::fs::copy(&old_token, &token)?;
    }
    let db = root.join("state.sqlite");
    let old_db = legacy.join("state.sqlite");
    if db.exists() || !old_db.exists() {
        return Ok(());
    }
    // A consistent copy that includes whatever the old WAL still holds,
    // published by an atomic rename: a crash leaves no database or all of it.
    let staged = root.join("state.sqlite.migrating");
    let _ = std::fs::remove_file(&staged);
    // Read-only, so closing it never checkpoints or rewrites the old files.
    Connection::open_with_flags(&old_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
        .execute("VACUUM INTO ?1", [staged.to_string_lossy()])?;
    {
        let copy = Connection::open(&staged)?;
        let check: String = copy.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        ensure!(
            check == "ok",
            "migrated Zapper state failed its check: {check}"
        );
    }
    std::fs::File::open(&staged)?.sync_all()?;
    std::fs::rename(&staged, &db)?;
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(root) {
        let _ = dir.sync_all();
    }
    // The old database, WAL and token are kept for downgrade; they go with the legacy module.
    Ok(())
}

/// Rename pre-rename tables and their `zot` column, in one transaction.
/// A table that already exists under its new name is left as it is.
pub(super) fn rename_tables(db: &mut Connection) -> Result<()> {
    let tx = db.transaction()?;
    let exists = |table: &str| -> rusqlite::Result<bool> {
        tx.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
    };
    let has_legacy_column = |table: &str| -> rusqlite::Result<bool> {
        tx.query_row(
            "SELECT 1 FROM pragma_table_info(?1) WHERE name=?2",
            [table, LEGACY_COLUMN],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
    };
    for (old, new) in TABLES {
        if exists(old)? && !exists(new)? {
            tx.execute_batch(&format!("ALTER TABLE {old} RENAME TO {new}"))?;
        }
        if has_legacy_column(new)? {
            tx.execute_batch(&format!(
                "ALTER TABLE {new} RENAME COLUMN {LEGACY_COLUMN} TO zapper"
            ))?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Zappers, tests::config};
    use zuse_protocol::zappers::*;

    /// The schema every engine before the rename created, verbatim.
    const PRE_RENAME_SCHEMA: &str = "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
        CREATE TABLE zots(id TEXT PRIMARY KEY, state TEXT NOT NULL);
        CREATE TABLE zot_receipts(id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL);
        CREATE TABLE zot_workers(chat TEXT NOT NULL, device TEXT NOT NULL, zot TEXT NOT NULL,
            PRIMARY KEY(device,chat));
        CREATE TABLE zot_files(zot TEXT NOT NULL, path TEXT NOT NULL, version INTEGER NOT NULL,
            content TEXT NOT NULL, PRIMARY KEY(zot,path,version));
        CREATE TABLE zot_events(id TEXT PRIMARY KEY, zot TEXT NOT NULL, worker TEXT NOT NULL,
            kind TEXT NOT NULL, summary TEXT NOT NULL, acknowledged INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE zot_outbox(id TEXT PRIMARY KEY, zot TEXT NOT NULL, chat TEXT NOT NULL,
            action TEXT NOT NULL, phase TEXT NOT NULL DEFAULT 'pending', error TEXT);
        CREATE TABLE zot_delivery(id TEXT PRIMARY KEY, baseline TEXT, sent_at INTEGER NOT NULL);
        CREATE TABLE zot_notifications(id TEXT PRIMARY KEY);
        CREATE TABLE zot_turn_summaries(id TEXT PRIMARY KEY, summary TEXT NOT NULL);
        CREATE TABLE zot_worker_context(chat TEXT PRIMARY KEY, home TEXT NOT NULL, zot TEXT NOT NULL, revision INTEGER NOT NULL, instructions TEXT NOT NULL);
        CREATE TABLE zot_binding_versions(chat TEXT NOT NULL, home TEXT NOT NULL, zot TEXT NOT NULL, revision INTEGER NOT NULL, PRIMARY KEY(chat,home,zot));
        CREATE TABLE zot_releases(chat TEXT NOT NULL, device TEXT NOT NULL, zot TEXT NOT NULL, revision INTEGER NOT NULL, PRIMARY KEY(chat,device,zot));
        CREATE TABLE zot_worker_holds(zot TEXT NOT NULL, chat TEXT NOT NULL, reason TEXT NOT NULL, PRIMARY KEY(zot,chat));
        CREATE TABLE zot_targets(zot TEXT NOT NULL, target TEXT NOT NULL, revision INTEGER NOT NULL, PRIMARY KEY(zot,target));
        CREATE TABLE zot_coordinator_turns(chat TEXT PRIMARY KEY, zot TEXT NOT NULL, ledger TEXT NOT NULL);
        CREATE TABLE zot_queue(id TEXT PRIMARY KEY);
        ALTER TABLE zot_worker_holds ADD COLUMN stamped INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE zot_worker_holds ADD COLUMN turn TEXT;";

    fn saved_zapper() -> Zapper {
        Zapper {
            id: "release".into(),
            revision: 4,
            activity_revision: 0,
            config: config(),
            status: ZapperStatus::Active,
            coordinator_chat_id: "zot-release-3".into(),
            previous_coordinator_chats: vec!["zot-release-0".into()],
            workers: vec![ZapperWorker {
                chat_id: "zot-worker-delegate-1".into(),
                device_id: "local".into(),
                space_id: "repo".into(),
                objective: "Fix the build".into(),
                followups: 1,
                queue_position: None,
            }],
            decisions: vec![],
            issues: vec![],
        }
    }

    /// A profile as an engine from before the rename left it: `zots/` with
    /// its token, a coordinator folder, and a database whose last writes are
    /// still only in the WAL (the engine stopped without checkpointing).
    fn pre_rename_profile(profile: &std::path::Path) -> (String, String) {
        let legacy = profile.join("zots");
        std::fs::create_dir_all(legacy.join("release")).unwrap();
        std::fs::write(legacy.join("release/notes.txt"), "kept").unwrap();
        let token = "ab".repeat(32);
        std::fs::write(legacy.join("control-token"), &token).unwrap();
        let zapper = saved_zapper();
        let retried = super::super::tests::command(
            "release",
            "configure-1",
            ZapperAction::Configure { config: config() },
        );
        let legacy_input = serde_json::to_string(&retried)
            .unwrap()
            .replace("\"zapperId\"", "\"zotId\"");
        assert!(legacy_input.contains("\"zotId\":\"release\""));
        let db = rusqlite::Connection::open(legacy.join("state.sqlite")).unwrap();
        db.execute_batch(PRE_RENAME_SCHEMA).unwrap();
        let state = serde_json::to_string(&zapper).unwrap();
        db.execute("INSERT INTO zots VALUES('release',?1)", [&state])
            .unwrap();
        db.execute(
            "INSERT INTO zot_receipts VALUES('configure-1',?1,?2)",
            [
                &legacy_input,
                &serde_json::to_string(&Some(&zapper)).unwrap(),
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO zot_workers VALUES('zot-worker-delegate-1','local','release')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO zot_files VALUES('release','notes.md',2,'Ship on Friday')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO zot_events(id,zot,worker,kind,summary) VALUES('e1','release','zot-worker-delegate-1','report','Status: done')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO zot_worker_holds(zot,chat,reason) VALUES('release','zot-worker-delegate-1','user: Stopped')",
            [],
        )
        .unwrap();
        // Skip the checkpoint a clean close would run.
        std::mem::forget(db);
        assert!(legacy.join("state.sqlite-wal").metadata().unwrap().len() > 0);
        (token, legacy_input)
    }

    #[test]
    fn pre_rename_state_loads_unchanged_and_migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path();
        let (token, _) = pre_rename_profile(profile);

        for _ in 0..2 {
            let store = Zappers::open(profile, "local").unwrap();
            // The Zapper, its chat ids, workers, files and events, as saved.
            let zapper = store.get("release").unwrap();
            assert_eq!(zapper.coordinator_chat_id, "zot-release-3");
            assert_eq!(zapper.previous_coordinator_chats, ["zot-release-0"]);
            assert_eq!(zapper.workers, saved_zapper().workers);
            assert_eq!(store.list().unwrap().len(), 1);
            let files = store.files("release", true).unwrap();
            assert!(
                files.iter().any(|f| f.path == "notes.md"
                    && f.version == 2
                    && f.content == "Ship on Friday")
            );
            let events = store.events("release").unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].zapper_id, "release");
            assert_eq!(events[0].worker_chat_id, "zot-worker-delegate-1");
            assert_eq!(
                store.coordinator("zot-release-3").unwrap().map(|z| z.id),
                Some("release".into())
            );
            // A command retried across the upgrade still matches its receipt.
            let retried = super::super::tests::command(
                "release",
                "configure-1",
                ZapperAction::Configure { config: config() },
            );
            assert!(store.receipt(&retried).unwrap().is_some());
            assert_eq!(store.command(retried).unwrap().unwrap().revision, 4);
            // Tools keep their token; the coordinator keeps its folder.
            assert!(store.authorize(&token));
            assert_eq!(
                store.workspace("release").unwrap(),
                profile.join("zots/release")
            );
            assert!(profile.join("zots/release/notes.txt").exists());
            drop(store);
        }
        // An older build still opens its pre-upgrade state: database, WAL
        // and token are untouched.
        assert!(
            profile
                .join("zots/state.sqlite-wal")
                .metadata()
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(
            std::fs::read_to_string(profile.join("zots/control-token")).unwrap(),
            token
        );
        let old = rusqlite::Connection::open(profile.join("zots/state.sqlite")).unwrap();
        let state: String = old
            .query_row("SELECT state FROM zots WHERE id='release'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Zapper>(&state).unwrap(),
            saved_zapper()
        );
        let counts: (i64, i64, i64) = old
            .query_row(
                "SELECT (SELECT COUNT(*) FROM zot_files WHERE zot='release'),
                        (SELECT COUNT(*) FROM zot_events WHERE zot='release'),
                        (SELECT COUNT(*) FROM zot_receipts)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 1, 1));
        drop(old);

        assert!(profile.join("zappers/state.sqlite").exists());
        let db = rusqlite::Connection::open(profile.join("zappers/state.sqlite")).unwrap();
        let tables: Vec<String> = db
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(tables.len(), super::TABLES.len() + 1);
        assert!(tables.iter().any(|table| table == "zapper_input_reports"));
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM zapper_input_reports", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0,
        );
        for (old, new) in super::TABLES {
            assert!(!tables.iter().any(|t| t == old), "{old} survived");
            assert!(tables.iter().any(|t| t == new), "{new} missing");
            let legacy_columns: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name=?2",
                    [*new, super::LEGACY_COLUMN],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(legacy_columns, 0, "{new}");
        }
    }

    #[test]
    fn new_zappers_use_new_names_beside_migrated_ones() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path();
        pre_rename_profile(profile);
        let store = Zappers::open(profile, "local").unwrap();
        let created = store
            .command(super::super::tests::command(
                "fresh",
                "create-fresh",
                ZapperAction::Create { config: config() },
            ))
            .unwrap()
            .unwrap();
        assert_eq!(created.coordinator_chat_id, "zapper-fresh-0");
        assert_eq!(
            store.workspace("fresh").unwrap(),
            profile.join("zappers/fresh")
        );
        // Discovery stays readable where older SDKs look for it.
        store.publish_discovery(4242).unwrap();
        assert!(profile.join("zappers/control.json").exists());
        assert!(profile.join("zots/control.json").exists());
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn fresh_profiles_have_no_legacy_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Zappers::open(dir.path(), "local").unwrap();
        store.publish_discovery(4242).unwrap();
        assert!(!dir.path().join("zots").exists());
        assert!(dir.path().join("zappers/control.json").exists());
    }
}
