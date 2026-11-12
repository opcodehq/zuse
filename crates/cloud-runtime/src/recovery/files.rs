//! Capture and journal installation never open a writable engine or replay a command.
use super::*;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::{
    collections::BTreeMap,
    fs::{self, Metadata},
    path::Component,
};
use tar::{Archive, Builder, EntryType, Header};
const ROOTS: [&str; 5] = [
    "data",
    "repositories",
    "claude-projects",
    "claude-file-history",
    "codex",
];
pub(super) fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}
pub(super) fn private_directory(path: &Path) -> Result<()> {
    for parent in path.ancestors() {
        if let Ok(meta) = fs::symlink_metadata(parent) {
            ensure!(!meta.file_type().is_symlink(), "recovery_directory_symlink");
        }
    }
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "recovery_directory_symlink"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn roots(layout: &Layout) -> [(String, PathBuf); 5] {
    [
        ("data".into(), layout.data.clone()),
        ("repositories".into(), layout.repositories.clone()),
        (
            "claude-projects".into(),
            layout.home.join(".claude/projects"),
        ),
        (
            "claude-file-history".into(),
            layout.home.join(".claude/file-history"),
        ),
        ("codex".into(), layout.home.join(".codex")),
    ]
}
fn resolved(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("recovery_path_invalid"))?;
    Ok(resolved(parent)?.join(
        path.file_name()
            .ok_or_else(|| anyhow::anyhow!("recovery_path_invalid"))?,
    ))
}
pub(super) fn validate_layout(layout: &Layout) -> Result<()> {
    for path in [
        &layout.data,
        &layout.repositories,
        &layout.home,
        &layout.work,
    ] {
        ensure!(
            path.is_absolute()
                && path
                    .components()
                    .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
            "recovery_layout_invalid"
        );
    }
    ensure!(layout.home.is_dir(), "recovery_home_missing");
    let work = resolved(&layout.work)?;
    let paths = roots(layout)
        .into_iter()
        .map(|(_, p)| resolved(&p))
        .collect::<Result<Vec<_>>>()?;
    for (index, p) in paths.iter().enumerate() {
        ensure!(
            !work.starts_with(p) && !p.starts_with(&work),
            "recovery_work_directory_overlaps"
        );
        for other in &paths[index + 1..] {
            ensure!(
                !p.starts_with(other) && !other.starts_with(p),
                "recovery_roots_overlap"
            );
        }
    }
    Ok(())
}
fn allowed(root: &str, relative: &Path) -> bool {
    let Some(first) = relative
        .components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
    else {
        return relative.as_os_str().is_empty() || root != "codex";
    };
    if first == ".zuse-archive" {
        return false;
    }
    if root == "data"
        && matches!(
            first,
            ".cloud-archive"
                | ".runtime-memory"
                | ".runtime-supervision"
                | "cloud-runtime-supervision.json"
                | "cloud-runtime-supervisor-recovery.json"
                | "cloud-runtime-supervisor-repairs.json"
                | "cloud-runtime-supervisor-recreation.json"
                | ".runtime-recreation"
                | ".workspace-runtime.lock"
                | "engine.lock"
                | "engine.lock.pid"
                | ".node-compile-cache"
                | "cloud-runtime-identity.json"
                | "cloud-runtime-credential"
                | "github-broker.json"
                | "rust-runtime-storage.json"
                | "rust-recovery-restored.json"
                | "session.json"
                | "model-accounts"
        )
    {
        return false;
    }
    if root == "data" && first.starts_with(".runtime-write-") {
        return false;
    }
    if root == "data" {
        // Device/account authorities may live at the device root or below a profile.
        // Preserve public device IDs and conversation journals, not reusable login or pairing keys.
        let components = relative.components().collect::<Vec<_>>();
        let index = match first {
            "orgs" => 3,
            "profiles" => 2,
            _ => 0,
        };
        let private = components
            .get(index)
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|name| {
                matches!(
                    name,
                    "account"
                        | "pairing"
                        | "managed-access"
                        | "relay-access.json"
                        | "browser-access-token"
                        | "session.json"
                        | "model-accounts"
                ) || name.starts_with(".browser-token-")
            });
        if private {
            return false;
        }
    }
    if root == "codex" {
        return matches!(
            first,
            "sessions" | "archived_sessions" | "session_index.jsonl"
        ) || first
            .strip_prefix("state_")
            .and_then(|s| s.split_once(".sqlite"))
            .is_some_and(|(n, suffix)| {
                !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && matches!(suffix, "" | "-wal" | "-shm")
            });
    }
    true
}
fn link_inside(relative: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth = relative
        .parent()
        .map(|p| p.components().count())
        .unwrap_or(0);
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return false,
        }
    }
    true
}
#[derive(PartialEq, Eq)]
struct Fingerprint(u64, u64, u64, i64, i64, u32);
fn fingerprint(m: &Metadata) -> Fingerprint {
    if m.is_dir() {
        // A directory's size/mtime also move when an excluded entry (a lock,
        // a supervisor record replaced by rename) changes. Its captured
        // contents are compared by name instead; see `Seen::unchanged`.
        return Fingerprint(m.dev(), m.ino(), 0, 0, 0, m.mode());
    }
    Fingerprint(
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.mode(),
    )
}
/// One captured path and what must still hold for the archive to be consistent.
pub(super) struct Seen {
    path: PathBuf,
    before: Fingerprint,
    /// For a directory: the sorted names of its captured entries.
    entries: Option<Vec<std::ffi::OsString>>,
}
fn captured_entries(root: &str, path: &Path, relative: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut names = fs::read_dir(path)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|name| allowed(root, &relative.join(name)))
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}
impl Seen {
    pub(super) fn observe(root: &str, base: &Path, relative: &Path) -> Result<Self> {
        let path = base.join(relative);
        let meta = fs::symlink_metadata(&path)?;
        let entries = if meta.is_dir() {
            Some(captured_entries(root, &path, relative)?)
        } else {
            None
        };
        Ok(Self {
            path,
            before: fingerprint(&meta),
            entries,
        })
    }
    /// Re-read after capture. A directory is unchanged when its identity and
    /// captured entry names are; excluded entries may come and go.
    pub(super) fn unchanged(&self, root: &str, relative: &Path) -> Result<bool> {
        if fingerprint(&fs::symlink_metadata(&self.path)?) != self.before {
            return Ok(false);
        }
        Ok(match &self.entries {
            Some(names) => captured_entries(root, &self.path, relative)? == *names,
            None => true,
        })
    }
}
/// Partial archives from earlier attempts. Only one capture runs per
/// operation (runner lock), so any left behind is abandoned.
pub(super) fn remove_partial_captures(path: &Path) -> Result<()> {
    let (Some(directory), Some(stem)) = (path.parent(), path.file_stem()) else {
        return Ok(());
    };
    let prefix = format!("{}.", stem.to_string_lossy());
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".next") && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
/// Removes a partial archive unless the capture completed.
struct Partial<'a>(&'a Path, bool);
impl Drop for Partial<'_> {
    fn drop(&mut self) {
        if !self.1 {
            let _ = fs::remove_file(self.0);
        }
    }
}
pub(super) fn capture(layout: &Layout, path: &Path) -> Result<()> {
    ensure!(
        layout.data.is_dir() && layout.repositories.is_dir(),
        "recovery_required_root_missing"
    );
    // Failed attempts must never accumulate multi-GB partial files.
    remove_partial_captures(path)?;
    let next = path.with_extension(format!("{}.next", uuid::Uuid::new_v4()));
    let out = private_file(&next)?;
    let mut partial = Partial(&next, false);
    let mut tar = Builder::new(GzEncoder::new(out, Compression::default()));
    let mut observed = vec![];
    fn add(
        root: &str,
        base: &Path,
        relative: &Path,
        tar: &mut Builder<GzEncoder<File>>,
        observed: &mut Vec<(String, PathBuf, Seen)>,
    ) -> Result<()> {
        if !allowed(root, relative) {
            return Ok(());
        }
        let path = base.join(relative);
        let seen = Seen::observe(root, base, relative)?;
        let meta = fs::symlink_metadata(&path)?;
        let before = fingerprint(&meta);
        let mut header = Header::new_gnu();
        header.set_uid(0);
        header.set_gid(0);
        header.set_mode(meta.mode() & 0o777);
        header.set_mtime(meta.mtime().max(0) as u64);
        let name = Path::new(root).join(relative);
        if meta.is_dir() {
            header.set_entry_type(EntryType::Directory);
            header.set_size(0);
            tar.append_data(&mut header, &name, std::io::empty())?;
            for entry in seen.entries.clone().unwrap_or_default() {
                add(root, base, &relative.join(entry), tar, observed)?;
            }
        } else if meta.is_file() {
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?;
            ensure!(
                fingerprint(&file.metadata()?) == before,
                "recovery_source_changed"
            );
            header.set_entry_type(EntryType::Regular);
            header.set_size(meta.len());
            tar.append_data(&mut header, &name, &mut file)?;
            ensure!(
                fingerprint(&file.metadata()?) == before,
                "recovery_source_changed"
            );
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            ensure!(link_inside(relative, &target), "recovery_external_symlink");
            header.set_entry_type(EntryType::Symlink);
            header.set_size(0);
            tar.append_link(&mut header, &name, target)?;
        } else {
            anyhow::bail!("recovery_unsupported_entry")
        }
        observed.push((root.to_owned(), relative.to_owned(), seen));
        Ok(())
    }
    for (root, source) in roots(layout) {
        match fs::canonicalize(&source) {
            Ok(source) => {
                ensure!(source.is_dir(), "recovery_root_not_directory");
                add(&root, &source, Path::new(""), &mut tar, &mut observed)?;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && !matches!(root.as_str(), "data" | "repositories") => {}
            Err(e) => return Err(e.into()),
        }
    }
    let out = tar.into_inner()?.finish()?;
    out.sync_all()?;
    for (root, relative, seen) in observed {
        ensure!(seen.unchanged(&root, &relative)?, "recovery_source_changed");
    }
    fs::rename(&next, path)?;
    partial.1 = true;
    File::open(&layout.work)?.sync_all()?;
    Ok(())
}
pub(super) fn extract(archive: &Path, stage: &Path) -> Result<()> {
    // Validate every logical member before any extraction; a symlink cannot be an extraction parent.
    let mut kinds = BTreeMap::new();
    let mut tar = Archive::new(GzDecoder::new(File::open(archive)?));
    for entry in tar.entries()? {
        let entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(relative(&path), "recovery_archive_path_invalid");
        let root = path
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_str()
            .unwrap_or("");
        ensure!(ROOTS.contains(&root), "recovery_archive_root_invalid");
        let rel = path.strip_prefix(root)?;
        ensure!(allowed(root, rel), "recovery_archive_credentials_forbidden");
        let kind = entry.header().entry_type();
        if rel.as_os_str().is_empty() {
            ensure!(kind.is_dir(), "recovery_archive_root_invalid");
        }
        ensure!(
            kind.is_file() || kind.is_dir() || kind.is_symlink(),
            "recovery_archive_entry_invalid"
        );
        if kind.is_symlink() {
            let target = entry
                .link_name()?
                .ok_or_else(|| anyhow::anyhow!("recovery_archive_link_invalid"))?;
            ensure!(link_inside(rel, &target), "recovery_archive_link_invalid");
        }
        ensure!(
            kinds.insert(path, kind).is_none(),
            "recovery_archive_duplicate_path"
        );
    }
    for path in kinds.keys() {
        for parent in path.ancestors().skip(1) {
            if let Some(kind) = kinds.get(parent) {
                ensure!(kind.is_dir(), "recovery_archive_link_parent");
            }
        }
    }
    for root in ["data", "repositories"] {
        ensure!(
            kinds.get(Path::new(root)).is_some_and(|k| k.is_dir()),
            "recovery_required_root_missing"
        );
    }
    if stage.exists() {
        ensure!(
            !fs::symlink_metadata(stage)?.file_type().is_symlink(),
            "recovery_stage_invalid"
        );
        fs::remove_dir_all(stage)?;
    }
    private_directory(stage)?;
    let mut tar = Archive::new(GzDecoder::new(File::open(archive)?));
    tar.set_preserve_permissions(false);
    tar.set_preserve_ownerships(false);
    for entry in tar.entries()? {
        ensure!(entry?.unpack_in(stage)?, "recovery_archive_path_invalid");
    }
    sync_tree(stage)?;
    Ok(())
}
fn database(path: &Path) -> Result<rusqlite::Connection> {
    ensure!(
        path.is_file() && !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "recovery_database_missing"
    );
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let values = db
        .prepare("PRAGMA integrity_check")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(values == ["ok"], "recovery_database_corrupt");
    Ok(db)
}
pub(super) fn validate(data: &Path, binding: &Binding) -> Result<()> {
    if let Some(storage) = &binding.rust_storage {
        let db = database(&data.join(&storage.database))?;
        let ids:(String,String,String)=db.query_row("SELECT workspace_id,account_id,storage_incarnation_id FROM zuse_cloud_storage_identity WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        ensure!(
            ids == (
                binding.workspace_id.clone(),
                storage.account_id.clone(),
                storage.storage_incarnation_id.clone()
            ),
            "recovery_storage_identity_mismatch"
        );
        let bytes: Vec<u8> = db.query_row(
            "SELECT bytes FROM snapshots WHERE doc_id=?1",
            [zuse_doc::REGISTRY_DOC_ID],
            |r| r.get(0),
        )?;
        let registry = zuse_doc::RegistryDoc::from_bytes(&bytes, "archive-validation")?;
        ensure!(
            registry
                .read_chats()?
                .iter()
                .all(|chat| chat.config.as_ref().is_none_or(|config| matches!(
                    config.harness,
                    zuse_protocol::HarnessId::Zuse
                        | zuse_protocol::HarnessId::Mock
                        | zuse_protocol::HarnessId::ClaudeCode
                        | zuse_protocol::HarnessId::Codex
                ))),
            "recovery_native_agent_unsupported"
        );
        ensure!(
            registry.chat(&binding.chat_id)?.is_some()
                && registry.chat(&binding.session_id)?.is_some(),
            "recovery_conversation_missing"
        );
        let bytes: Vec<u8> = db.query_row(
            "SELECT bytes FROM snapshots WHERE doc_id=?1",
            [&binding.session_id],
            |r| r.get(0),
        )?;
        let raw = loro::LoroDoc::new();
        raw.import(&bytes)?;
        let session = zuse_doc::SessionDoc::from_doc(raw);
        ensure!(
            session.chat_id().as_deref() == Some(binding.session_id.as_str()),
            "recovery_session_identity_mismatch"
        );
        session.read_entries()?;
    } else {
        let db = database(&data.join("zuse.sqlite"))?;
        let unsupported: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE provider_id IS NULL OR provider_id NOT IN ('claude','codex'))",
            [],
            |r| r.get(0),
        )?;
        ensure!(!unsupported, "recovery_native_agent_unsupported");
        for (table, id) in [
            ("chats", &binding.chat_id),
            ("sessions", &binding.session_id),
        ] {
            let found: bool = db.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
                [id],
                |r| r.get(0),
            )?;
            ensure!(found, "recovery_conversation_missing");
        }
    }
    let primary = data.join(
        binding
            .rust_storage
            .as_ref()
            .map(|s| s.database.as_path())
            .unwrap_or(Path::new("zuse.sqlite")),
    );
    check_databases(data, Some(&primary))?;
    Ok(())
}
fn check_databases(path: &Path, skip: Option<&Path>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.file_type()?;
        if meta.is_dir() {
            check_databases(&entry.path(), skip)?
        } else if meta.is_file() && skip != Some(entry.path().as_path()) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".sqlite") || name.ends_with(".sqlite3") {
                database(&entry.path())?;
            }
        }
    }
    Ok(())
}
pub(super) fn validate_stage(stage: &Path, binding: &Binding) -> Result<()> {
    validate(&stage.join("data"), binding)?;
    let codex = stage.join("codex");
    if codex.is_dir() {
        check_databases(&codex, None)?;
    }
    Ok(())
}

pub(super) fn validate_installed(layout: &Layout, binding: &Binding) -> Result<()> {
    validate(&layout.data, binding)?;
    let codex = layout.home.join(".codex");
    if codex.is_dir() {
        check_databases(&codex, None)?;
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?
        }
        File::open(path)?.sync_all()?;
    } else if meta.is_file() {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    incoming: bool,
    previous: bool,
    done: bool,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    binding: Binding,
    entries: Vec<Entry>,
}
fn entry_name(entry: &Entry) -> Result<std::ffi::OsString> {
    let bytes = URL_SAFE_NO_PAD.decode(&entry.name)?;
    ensure!(!bytes.contains(&0), "recovery_install_journal_invalid");
    let name = std::ffi::OsString::from_vec(bytes);
    ensure!(
        relative(Path::new(&name)) && Path::new(&name).components().count() == 1,
        "recovery_install_journal_invalid"
    );
    Ok(name)
}
fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
fn rename(source: &Path, target: &Path) -> Result<()> {
    ensure!(!present(target), "recovery_install_destination_occupied");
    fs::rename(source, target)?;
    File::open(source.parent().unwrap())?.sync_all()?;
    File::open(target.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn copy(source: &Path, target: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
        fs::set_permissions(target, meta.permissions())?;
        File::open(target)?.sync_all()?;
    } else if meta.is_file() {
        let mut src = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(source)?;
        let mut out = private_file(target)?;
        std::io::copy(&mut src, &mut out)?;
        fs::set_permissions(target, meta.permissions())?;
        out.sync_all()?;
    } else if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(source)?, target)?;
    } else {
        anyhow::bail!("recovery_install_entry_invalid")
    }
    Ok(())
}
pub(super) fn install(layout: &Layout, binding: &Binding, stage: &Path) -> Result<()> {
    // A retry may see only the database installed by this operation. A new authoritative database is never replaced.
    fn databases(path: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name() == ".zuse-archive" {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                databases(&entry.path(), found)?;
            } else if matches!(
                entry.file_name().to_str(),
                Some("docs.sqlite3" | "zuse.sqlite")
            ) {
                found.push(entry.path());
            }
        }
        Ok(())
    }
    let database_relative = binding
        .rust_storage
        .as_ref()
        .map(|s| s.database.as_path())
        .unwrap_or(Path::new("zuse.sqlite"));
    let expected_database = layout.data.join(database_relative);
    let mut found = vec![];
    databases(&layout.data, &mut found)?;
    ensure!(
        found.iter().all(|path| *path == expected_database),
        "recovery_existing_database"
    );
    if present(&expected_database) {
        let operation = layout
            .data
            .join(".zuse-archive")
            .join(&binding.operation_id);
        let journal: Journal = serde_json::from_slice(
            &fs::read(operation.join("entries.json"))
                .map_err(|_| anyhow::anyhow!("recovery_existing_database"))?,
        )?;
        let first = database_relative.components().next().unwrap().as_os_str();
        ensure!(
            journal.binding == *binding
                && journal
                    .entries
                    .iter()
                    .any(|e| entry_name(e).is_ok_and(|name| name == first) && e.incoming)
                && !present(&operation.join("incoming").join(first)),
            "recovery_existing_database"
        );
        validate(&layout.data, binding)?;
    }
    // One durable preflight covers all roots, before even repositories can change.
    let preflight = layout.work.join("installing");
    if !preflight.exists() {
        let expected = layout.data.join(
            binding
                .rust_storage
                .as_ref()
                .map(|s| s.database.as_path())
                .unwrap_or(Path::new("zuse.sqlite")),
        );
        ensure!(!present(&expected), "recovery_existing_database");
        write(&preflight, &serde_json::to_vec(binding)?)?;
    }
    ensure!(
        serde_json::from_slice::<Binding>(&fs::read(&preflight)?)? == *binding,
        "recovery_install_identity_mismatch"
    );
    for (root, target) in roots(layout) {
        let source = stage.join(&root);
        if !source.exists() && !matches!(root.as_str(), "data" | "repositories") {
            continue;
        }
        fs::create_dir_all(&target)?;
        let target = fs::canonicalize(target)?;
        let operation = target.join(".zuse-archive").join(&binding.operation_id);
        private_directory(&operation)?;
        let journal = operation.join("entries.json");
        let incoming = operation.join("incoming");
        let previous = operation.join("previous");
        private_directory(&previous)?;
        let mut state: Journal = if journal.exists() {
            serde_json::from_slice(&fs::read(&journal)?)?
        } else {
            if !incoming.exists() {
                let copying = operation.join(format!("copying-{}", uuid::Uuid::new_v4()));
                copy(&source, &copying)?;
                sync_tree(&copying)?;
                rename(&copying, &incoming)?;
            }
            let mut names = BTreeMap::new();
            for (dir, from_archive) in [(&target, false), (&incoming, true)] {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let file_name = entry.file_name();
                    if file_name == ".zuse-archive" || !allowed(&root, Path::new(&file_name)) {
                        continue;
                    }
                    let name = URL_SAFE_NO_PAD.encode(file_name.as_bytes());
                    let item = names.entry(name.clone()).or_insert(Entry {
                        name,
                        incoming: false,
                        previous: false,
                        done: false,
                    });
                    if from_archive {
                        item.incoming = true
                    } else {
                        item.previous = true
                    }
                }
            }
            let state = Journal {
                binding: binding.clone(),
                entries: names.into_values().collect(),
            };
            write(&journal, &serde_json::to_vec(&state)?)?;
            state
        };
        ensure!(
            state.binding == *binding,
            "recovery_install_identity_mismatch"
        );
        for index in 0..state.entries.len() {
            let entry = &state.entries[index];
            let name = entry_name(entry)?;
            if entry.done {
                continue;
            }
            let from = incoming.join(&name);
            let to = target.join(&name);
            let saved = previous.join(&name);
            if present(&from) {
                if present(&to) {
                    ensure!(
                        entry.previous && !present(&saved),
                        "recovery_install_destination_occupied"
                    );
                    rename(&to, &saved)?;
                }
                rename(&from, &to)?;
            } else if entry.incoming {
                ensure!(present(&to), "recovery_install_entry_missing");
            } else if present(&to) {
                ensure!(!present(&saved), "recovery_install_destination_occupied");
                rename(&to, &saved)?;
            } else {
                ensure!(present(&saved), "recovery_install_previous_missing");
            }
            state.entries[index].done = true;
            write(&journal, &serde_json::to_vec(&state)?)?;
        }
    }
    for (_, root) in roots(layout) {
        if root.exists() {
            sync_tree(&root)?;
        }
    }
    Ok(())
}
