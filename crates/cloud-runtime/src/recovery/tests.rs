use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[derive(Default)]
struct Memory {
    bytes: Mutex<HashMap<String, Vec<u8>>>,
    order: Mutex<Vec<String>>,
    fail_after: AtomicUsize,
    corrupt_reads: AtomicBool,
    reads: AtomicUsize,
}
#[async_trait]
impl Objects for Memory {
    async fn get(&self, name: &str) -> Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut bytes = self
            .bytes
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing"))?;
        if name != "manifest" && self.corrupt_reads.load(Ordering::SeqCst) {
            bytes[20] ^= 1;
        }
        Ok(bytes)
    }
    async fn put(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut order = self.order.lock().unwrap();
        if self.fail_after.load(Ordering::SeqCst) > 0
            && order.len() >= self.fail_after.load(Ordering::SeqCst)
        {
            anyhow::bail!("interrupted")
        }
        let mut objects = self.bytes.lock().unwrap();
        if let Some(prior) = objects.get(name) {
            ensure!(prior == bytes, "immutable");
        } else {
            objects.insert(name.into(), bytes.into());
        }
        order.push(name.into());
        Ok(())
    }
}
fn binding() -> Binding {
    Binding {
        operation_id: "operation".into(),
        workspace_id: "workspace".into(),
        chat_id: "thread".into(),
        session_id: "session".into(),
        rust_storage: Some(RustStorage {
            database: "orgs/workspace/account/docs.sqlite3".into(),
            account_id: "account".into(),
            storage_incarnation_id: "original-storage".into(),
        }),
    }
}
fn key() -> Key {
    Key::parse(&URL_SAFE_NO_PAD.encode([9; 32])).unwrap()
}
fn layout(base: &Path) -> Layout {
    let home = base.join("home");
    let data = base.join("data");
    let repositories = base.join("repositories");
    for path in [&home, &data, &repositories] {
        std::fs::create_dir_all(path).unwrap();
    }
    Layout {
        data,
        repositories,
        work: home.join(".zuse-recovery/operation"),
        home,
    }
}
fn put(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn chat(id: &str) -> zuse_protocol::Chat {
    serde_json::from_value(json!({"id":id,"deviceId":"original-device","title":"Retained conversation","archived":true,"cwd":"/home/repos/project","branch":"feature","checkoutId":null,"config":null,"lastMessagePreview":null,"lastMessageAt":null,"createdAt":"2026-10-23T00:00:00Z"})).unwrap()
}
fn message(index: usize) -> zuse_doc::SessionMessageEntry {
    serde_json::from_value(json!({"id":format!("message-{index}"),"role":"assistant","createdAt":index,"deviceId":"original-device","parts":[{"id":format!("part-{index}"),"kind":"text","text":format!("Retained message {index}") }]})).unwrap()
}
fn seed(layout: &Layout) -> rusqlite::Connection {
    let binding = binding();
    let database = layout
        .data
        .join(&binding.rust_storage.as_ref().unwrap().database);
    std::fs::create_dir_all(database.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE snapshots(doc_id TEXT PRIMARY KEY,bytes BLOB); CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER PRIMARY KEY,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT); INSERT INTO zuse_cloud_storage_identity VALUES(1,'workspace','account','original-storage'); CREATE TABLE processed_commands(command_id TEXT PRIMARY KEY); INSERT INTO processed_commands VALUES('already-executed');").unwrap();
    let mut registry = zuse_doc::RegistryDoc::new("original-device");
    registry.set_local_only(true);
    registry.upsert_chat(&chat("thread")).unwrap();
    registry.upsert_chat(&chat("session")).unwrap();
    db.execute(
        "INSERT INTO snapshots VALUES(?1,?2)",
        rusqlite::params![zuse_doc::REGISTRY_DOC_ID, registry.to_bytes().unwrap()],
    )
    .unwrap();
    let session = zuse_doc::SessionDoc::init("session").unwrap();
    for i in 0..350 {
        session.push_message(&message(i)).unwrap();
    }
    session.set_queue_paused(true).unwrap();
    db.execute(
        "INSERT INTO snapshots VALUES('session',?1)",
        [session.export_snapshot().unwrap()],
    )
    .unwrap();
    put(
        &layout.data.join("journals/root.json"),
        br#"{"pending_calls":["uncertain-tool"],"receipts":{"uncertain-tool":"started"}}"#,
    );
    put(
        &layout.data.join("uploads/attachment.bin"),
        b"original attachment",
    );
    for name in [
        "cloud-runtime-identity.json",
        "cloud-runtime-supervision.json",
        "cloud-runtime-supervisor-recovery.json",
        "cloud-runtime-supervisor-repairs.json",
        "cloud-runtime-supervisor-recreation.json",
        ".runtime-recreation/environment.json",
        "session.json",
        "model-accounts/key",
        "rust-runtime-storage.json",
        "account/session.json",
        "orgs/workspace/account/managed-access/identity.json",
        "orgs/workspace/account/pairing/peers.json",
        "orgs/workspace/account/relay-access.json",
        "orgs/workspace/account/browser-access-token",
    ] {
        put(&layout.data.join(name), b"OBSOLETE_ACCOUNT_CREDENTIAL");
    }
    put(
        &layout.repositories.join("project/.env"),
        b"RETAINED_USER_ENV=yes",
    );
    put(
        &layout.repositories.join("project/untracked.txt"),
        b"untracked work",
    );
    put(
        &layout
            .home
            .join(".claude/projects/project/native-session.jsonl"),
        b"original native continuation",
    );
    put(
        &layout.home.join(".codex/sessions/thread.jsonl"),
        b"original codex continuation",
    );
    put(
        &layout.home.join(".codex/auth.json"),
        b"REUSABLE_PROVIDER_CREDENTIAL",
    );
    std::fs::create_dir_all(layout.data.join(".runtime-memory")).unwrap();
    let socket =
        std::os::unix::net::UnixListener::bind(layout.data.join(".runtime-memory/wake.sock"))
            .unwrap();
    drop(socket); // Retained stale socket after a process-loss recovery.
    std::fs::create_dir_all(layout.data.join(".runtime-supervision")).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(
        layout.data.join(".runtime-supervision/control.sock"),
    )
    .unwrap();
    drop(socket);
    db
}
fn session(data: &Path) -> zuse_doc::SessionDoc {
    let db =
        rusqlite::Connection::open(data.join(binding().rust_storage.unwrap().database)).unwrap();
    let bytes: Vec<u8> = db
        .query_row(
            "SELECT bytes FROM snapshots WHERE doc_id='session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let raw = loro::LoroDoc::new();
    raw.import(&bytes).unwrap();
    zuse_doc::SessionDoc::from_doc(raw)
}
#[tokio::test]
async fn full_native_wal_round_trip_retains_history_journals_work_and_fresh_identity() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("machine");
    let layout = layout(&base);
    let db = seed(&layout);
    use std::os::unix::ffi::OsStringExt;
    let raw_name = std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec());
    put(&layout.repositories.join(&raw_name), b"original file bytes");
    let mut random = vec![0; CHUNK + 200000];
    OsRng.fill_bytes(&mut random);
    put(&layout.repositories.join("project/random.bin"), &random);
    fn git(path: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    let project = layout.repositories.join("project");
    git(&project, &["init", "--initial-branch=main"]);
    git(&project, &["config", "user.name", "Archive Test"]);
    git(
        &project,
        &["config", "user.email", "archive@example.invalid"],
    );
    put(&project.join("tracked.txt"), b"base");
    put(&project.join("deleted.txt"), b"delete this");
    put(&project.join(".gitignore"), b".env\n");
    git(
        &project,
        &["add", "tracked.txt", "deleted.txt", ".gitignore"],
    );
    git(&project, &["commit", "-m", "Retained base"]);
    git(&project, &["checkout", "-b", "preserved-feature"]);
    git(&project, &["tag", "retained-v1"]);
    put(&project.join("tracked.txt"), b"original modifications");
    std::fs::remove_file(project.join("deleted.txt")).unwrap();
    let side = layout.repositories.join("second");
    git(
        &project,
        &[
            "worktree",
            "add",
            "-b",
            "independent-worktree",
            side.to_str().unwrap(),
        ],
    );
    let original_head = git(&project, &["rev-parse", "HEAD"]);
    let helper = Recovery::acquire(binding(), layout.clone(), key()).unwrap();
    let objects = Memory::default();
    let manifest = helper.backup(&objects).await.unwrap();
    assert!(manifest.chunks.len() > 1);
    assert_eq!(objects.order.lock().unwrap().last().unwrap(), "manifest");
    assert_eq!(
        objects.reads.load(Ordering::SeqCst),
        0,
        "the guest must work with the original write-only upload capability"
    );
    let verifier_layout = super::tests::layout(&temp.path().join("control-plane-verifier"));
    let verifier = Recovery::acquire(binding(), verifier_layout.clone(), key()).unwrap();
    verifier.verify(&objects).await.unwrap();
    assert!(verifier_layout.work.join("verified").exists());
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        File::open(layout.work.join("capture.tar.gz")).unwrap(),
    ));
    let names: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|p| p.contains(".runtime-memory")
        || p.contains(".runtime-supervision")
        || p.contains("cloud-runtime-supervision")
        || p.contains("cloud-runtime-supervisor-recovery")
        || p.contains("cloud-runtime-supervisor-repairs")
        || p.contains("cloud-runtime-supervisor-recreation")
        || p.contains(".runtime-recreation")
        || p.contains("cloud-runtime-identity")
        || p.contains("model-accounts")
        || p == "codex/auth.json"
        || p.contains("/managed-access/")
        || p.contains("/pairing/")
        || p.contains("account/session.json")
        || p.ends_with("/browser-access-token")));
    drop(helper);
    drop(db);
    std::fs::rename(&base, temp.path().join("retired-machine")).unwrap();
    let target = super::tests::layout(&base);
    put(
        &target.data.join("cloud-runtime-identity.json"),
        b"FRESH_AUTHORITY_IDENTITY",
    );
    put(
        &target.repositories.join("image-file"),
        b"prepared image content",
    );
    let restored = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    restored.restore(&objects).await.unwrap();
    assert_eq!(
        std::fs::read(target.data.join("cloud-runtime-identity.json")).unwrap(),
        b"FRESH_AUTHORITY_IDENTITY"
    );
    assert_eq!(
        std::fs::read(target.repositories.join(&raw_name)).unwrap(),
        b"original file bytes"
    );
    assert!(!target.data.join("model-accounts").exists());
    assert!(!target.home.join(".codex/auth.json").exists());
    assert_eq!(
        std::fs::read(target.repositories.join("project/random.bin")).unwrap(),
        random
    );
    let project = target.repositories.join("project");
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), original_head);
    assert_eq!(
        git(&project, &["branch", "--show-current"]),
        "preserved-feature"
    );
    assert_eq!(
        git(&target.repositories.join("second"), &["rev-parse", "HEAD"]),
        original_head
    );
    assert_eq!(git(&project, &["rev-parse", "retained-v1"]), original_head);
    let diff = git(&project, &["diff", "--name-only"]);
    assert!(diff.contains("tracked.txt") && diff.contains("deleted.txt"));
    assert_eq!(
        std::fs::read(target.repositories.join("project/.env")).unwrap(),
        b"RETAINED_USER_ENV=yes"
    );
    assert_eq!(
        std::fs::read(target.data.join("uploads/attachment.bin")).unwrap(),
        b"original attachment"
    );
    assert!(
        target
            .repositories
            .join(".zuse-archive/operation/previous/image-file")
            .exists()
    );
    let doc = session(&target.data);
    assert_eq!(doc.read_entries().unwrap().len(), 350);
    assert_eq!(doc.queue_paused(), Some(true));
    doc.push_message(&message(350)).unwrap();
    assert_eq!(doc.chat_id().as_deref(), Some("session"));
    assert_eq!(doc.read_entries().unwrap().len(), 351);
    let database =
        rusqlite::Connection::open(target.data.join(binding().rust_storage.unwrap().database))
            .unwrap();
    let claims: i64 = database
        .query_row(
            "SELECT count(*) FROM processed_commands WHERE command_id='already-executed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(claims, 1);
    database
        .execute(
            "UPDATE snapshots SET bytes=?1 WHERE doc_id='session'",
            [doc.export_snapshot().unwrap()],
        )
        .unwrap();
    assert!(
        std::fs::read_to_string(target.data.join("journals/root.json"))
            .unwrap()
            .contains("uncertain-tool")
    );
    restored.restore(&objects).await.unwrap();
    assert_eq!(session(&target.data).read_entries().unwrap().len(), 351);
}
#[tokio::test]
async fn interrupted_upload_reuses_ciphertext_and_corruption_cannot_verify_or_replace_data() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let _db = seed(&layout);
    let helper = Recovery::acquire(binding(), layout.clone(), key()).unwrap();
    let objects = Memory::default();
    objects.fail_after.store(1, Ordering::SeqCst);
    assert!(helper.backup(&objects).await.is_err());
    assert!(!objects.bytes.lock().unwrap().contains_key("manifest"));
    let original = objects.bytes.lock().unwrap().clone();
    objects.fail_after.store(0, Ordering::SeqCst);
    let _manifest = helper.backup(&objects).await.unwrap();
    for (k, v) in original {
        assert_eq!(objects.bytes.lock().unwrap()[&k], v);
    }
    helper.verify(&objects).await.unwrap();
    objects.corrupt_reads.store(true, Ordering::SeqCst);
    assert!(
        helper.verify(&objects).await.is_err(),
        "verification must reread remote objects after a retry"
    );
    drop(helper);
    let target = super::tests::layout(&temp.path().join("replacement"));
    let restore = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    assert!(restore.restore(&objects).await.is_err());
    assert!(
        !target
            .data
            .join(binding().rust_storage.unwrap().database)
            .exists()
    );
}
#[tokio::test]
async fn storage_lock_and_existing_database_prevent_capture_or_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let db = seed(&layout);
    let helper = Recovery::acquire(binding(), layout.clone(), key()).unwrap();
    assert!(Recovery::acquire(binding(), layout.clone(), key()).is_err());
    let objects = Memory::default();
    helper.backup(&objects).await.unwrap();
    let before = file_digest(&layout.data.join(binding().rust_storage.unwrap().database)).unwrap();
    assert!(helper.restore(&objects).await.is_err());
    assert_eq!(
        file_digest(&layout.data.join(binding().rust_storage.unwrap().database)).unwrap(),
        before
    );
    drop(db);
}
#[tokio::test]
async fn external_links_are_refused_without_exporting_partial_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let _db = seed(&layout);
    std::os::unix::fs::symlink(
        "../../../../account-key",
        layout.repositories.join("project/unsafe-link"),
    )
    .unwrap();
    let helper = Recovery::acquire(binding(), layout.clone(), key()).unwrap();
    let objects = Memory::default();
    assert!(helper.backup(&objects).await.is_err());
    assert!(objects.bytes.lock().unwrap().is_empty());
    assert!(
        partial_captures(&layout.work).is_empty(),
        "a failed capture removes its partial archive"
    );
}
fn partial_captures(work: &Path) -> Vec<String> {
    std::fs::read_dir(work)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".next"))
        .collect()
}
#[tokio::test]
async fn capture_clears_partial_archives_left_by_earlier_attempts() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let _db = seed(&layout);
    let helper = Recovery::acquire(binding(), layout.clone(), key()).unwrap();
    for stale in ["capture.tar.0b7b8bd0.next", "capture.tar.136a640f.next"] {
        put(&layout.work.join(stale), vec![7; 4096]);
    }
    helper.backup(&Memory::default()).await.unwrap();
    assert!(partial_captures(&layout.work).is_empty());
    assert!(layout.work.join("capture.tar.gz").is_file());
}
#[test]
fn excluded_entries_churning_in_a_captured_directory_do_not_change_it() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    put(&layout.data.join("device-id"), "device");
    let seen = files::Seen::observe("data", &layout.data, Path::new("")).unwrap();
    // The supervisor replaces its recovery record by rename on every handoff.
    for _ in 0..3 {
        let next = layout.data.join(".runtime-write-supervisor-next");
        std::fs::write(&next, "{}").unwrap();
        std::fs::rename(
            &next,
            layout.data.join("cloud-runtime-supervisor-recovery.json"),
        )
        .unwrap();
        std::fs::write(layout.data.join("engine.lock"), "1").unwrap();
    }
    assert!(seen.unchanged("data", Path::new("")).unwrap());
    // A captured entry appearing mid-capture still invalidates it.
    put(&layout.data.join("late-journal.json"), "{}");
    assert!(!seen.unchanged("data", Path::new("")).unwrap());
}
#[test]
fn original_node_ciphertext_authenticates_only_in_its_operation_and_position() {
    use std::process::{Command, Stdio};
    let script = r#"const c=require('node:crypto');const k=Buffer.alloc(32,9),iv=Buffer.alloc(12,7);const x=c.createCipheriv('aes-256-gcm',k,iv);x.setAAD(Buffer.from('zuse-workspace-archive-v1\nworkspace\noperation\n0'));const b=Buffer.concat([iv,x.update(Buffer.from('original-node-archive-chunk')),x.final(),x.getAuthTag()]).toString('base64');process.stdout.write(b);"#;
    let output = Command::new("node")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        &*key().open(&binding(), 0, &output.stdout).unwrap(),
        b"original-node-archive-chunk"
    );
    assert!(key().open(&binding(), 1, &output.stdout).is_err());
    let mut other = binding();
    other.operation_id = "other".into();
    assert!(key().open(&other, 0, &output.stdout).is_err());
}

#[tokio::test]
async fn interrupted_install_retries_without_rewinding_installed_or_newer_work() {
    let temp = tempfile::tempdir().unwrap();
    let source = layout(&temp.path().join("source"));
    let _db = seed(&source);
    let backup = Recovery::acquire(binding(), source, key()).unwrap();
    let objects = Memory::default();
    backup.backup(&objects).await.unwrap();
    let target = layout(&temp.path().join("replacement"));
    let conflict = target.home.join(".claude/projects/.zuse-archive");
    std::fs::create_dir_all(conflict.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(temp.path(), &conflict).unwrap();
    let restore = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    assert!(restore.restore(&objects).await.is_err());
    assert_eq!(session(&target.data).read_entries().unwrap().len(), 350);
    assert!(!target.work.join("restored").exists());
    put(
        &target.repositories.join("project/untracked.txt"),
        b"newer work after interrupted install",
    );
    drop(restore);
    std::fs::remove_file(conflict).unwrap();
    let retry = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    retry.restore(&objects).await.unwrap();
    assert_eq!(
        std::fs::read(target.repositories.join("project/untracked.txt")).unwrap(),
        b"newer work after interrupted install"
    );
    assert_eq!(session(&target.data).read_entries().unwrap().len(), 350);
}
#[tokio::test]
async fn wrong_original_storage_identity_is_never_published() {
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let _db = seed(&layout);
    let mut wrong = binding();
    wrong.rust_storage.as_mut().unwrap().account_id = "other-account".into();
    let helper = Recovery::acquire(wrong, layout, key()).unwrap();
    let objects = Memory::default();
    assert!(helper.backup(&objects).await.is_err());
    assert!(objects.bytes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn imports_original_python_tar_and_node_cipher_without_initializing_a_rust_database() {
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let source = layout(&temp.path().join("legacy"));
    let db = rusqlite::Connection::open(source.data.join("zuse.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE chats(id TEXT);CREATE TABLE sessions(id TEXT,provider_id TEXT);INSERT INTO chats VALUES('thread');INSERT INTO sessions VALUES('session','codex');").unwrap();
    drop(db);
    put(
        &source.data.join("legacy-attachment.bin"),
        b"legacy user attachment",
    );
    put(&source.repositories.join(".env"), b"LEGACY_USER_ENV=yes");
    let archive = temp.path().join("legacy.tar.gz");
    let output=Command::new("python3").args(["-c","import sys,tarfile\nwith tarfile.open(sys.argv[1],'w:gz') as t:\n t.add(sys.argv[2],arcname='data')\n t.add(sys.argv[3],arcname='repositories')"]).arg(&archive).arg(&source.data).arg(&source.repositories).output().unwrap();
    assert!(output.status.success());
    let script = r#"const c=require('node:crypto'),fs=require('node:fs');const p=fs.readFileSync(process.argv[1]);const iv=Buffer.alloc(12,5),x=c.createCipheriv('aes-256-gcm',Buffer.alloc(32,9),iv);x.setAAD(Buffer.from('zuse-workspace-archive-v1\nworkspace\noperation\n0'));process.stdout.write(Buffer.concat([iv,x.update(p),x.final(),x.getAuthTag()]).toString('base64'));"#;
    let output = Command::new("node")
        .args(["-e", script])
        .arg(&archive)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    let hash = digest(&output.stdout);
    let manifest = ArchiveManifest {
        version: 1,
        workspace_id: "workspace".into(),
        operation_id: "operation".into(),
        chat_id: "thread".into(),
        session_id: "session".into(),
        chunks: vec![hash.clone()],
        archive_sha256: file_digest(&archive).unwrap(),
    };
    let objects = Memory::default();
    objects.bytes.lock().unwrap().insert(hash, output.stdout);
    objects
        .bytes
        .lock()
        .unwrap()
        .insert("manifest".into(), serde_json::to_vec(&manifest).unwrap());
    let mut legacy = binding();
    legacy.rust_storage = None;
    let target = layout(&temp.path().join("replacement"));
    let helper = Recovery::acquire(legacy, target.clone(), key()).unwrap();
    helper.restore(&objects).await.unwrap();
    assert_eq!(
        std::fs::read(target.data.join("legacy-attachment.bin")).unwrap(),
        b"legacy user attachment"
    );
    assert!(!target.data.join("docs.sqlite3").exists());
    assert_eq!(
        std::fs::read(target.repositories.join(".env")).unwrap(),
        b"LEGACY_USER_ENV=yes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the packaged ZUSE_CLOUD_TEST_GUEST_BIN"]
async fn packaged_helper_uses_write_only_upload_and_read_only_restore_capabilities() {
    use axum::{
        Router,
        body::Bytes,
        extract::{Path as RoutePath, State},
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use std::sync::Arc;
    async fn read(
        State(objects): State<Arc<Memory>>,
        RoutePath(name): RoutePath<String>,
        headers: HeaderMap,
    ) -> Result<Vec<u8>, StatusCode> {
        if headers.get("authorization").and_then(|h| h.to_str().ok()) != Some("Bearer restore-only")
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        objects.get(&name).await.map_err(|_| StatusCode::NOT_FOUND)
    }
    async fn upload(
        State(objects): State<Arc<Memory>>,
        RoutePath(name): RoutePath<String>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Result<StatusCode, StatusCode> {
        if headers.get("authorization").and_then(|h| h.to_str().ok()) != Some("Bearer upload-only")
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        objects
            .put(&name, &body)
            .await
            .map_err(|_| StatusCode::CONFLICT)?;
        Ok(StatusCode::NO_CONTENT)
    }
    let binary =
        std::env::var_os("ZUSE_CLOUD_TEST_GUEST_BIN").expect("build the packaged runtime first");
    let temp = tempfile::tempdir().unwrap();
    let source = layout(&temp.path().join("source"));
    let db = seed(&source);
    let objects = Arc::new(Memory::default());
    let app = Router::new()
        .route("/{name}", get(read).put(upload))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_WIRE))
        .with_state(objects.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    fn config(layout: &Layout, origin: &str, mode: &str, token: &str) -> String {
        let mut value = serde_json::to_value(binding()).unwrap();
        value["home"] = json!(layout.home);
        value["repositoriesPath"] = json!(layout.repositories);
        value["url"] = json!(origin);
        value["mode"] = json!(mode);
        value["token"] = json!(token);
        value["key"] = json!(URL_SAFE_NO_PAD.encode([9; 32]));
        value["expiresAtMs"] = json!(crate::now_ms() + 3600000);
        value.to_string()
    }
    let out = tokio::process::Command::new(&binary)
        .arg("--data-dir")
        .arg(&source.data)
        .arg("recover")
        .env(
            "ZUSE_ARCHIVE_CONFIG",
            config(&source, &origin, "backup", "upload-only"),
        )
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("workspace_archive_uploaded"));
    assert_eq!(objects.reads.load(Ordering::SeqCst), 0);
    let verifier =
        Recovery::acquire(binding(), layout(&temp.path().join("verifier")), key()).unwrap();
    verifier.verify(&*objects).await.unwrap();
    drop(db);
    let target = layout(&temp.path().join("replacement"));
    let out = tokio::process::Command::new(binary)
        .arg("--data-dir")
        .arg(&target.data)
        .arg("recover")
        .env(
            "ZUSE_ARCHIVE_CONFIG",
            config(&target, &origin, "restore", "restore-only"),
        )
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("workspace_archive_restored"));
    assert_eq!(session(&target.data).read_entries().unwrap().len(), 350);
    assert!(target.work.join("restored").exists());
    server.abort();
}

#[tokio::test]
async fn replacement_with_another_runtime_database_is_left_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let source = layout(&temp.path().join("source"));
    let _db = seed(&source);
    let helper = Recovery::acquire(binding(), source, key()).unwrap();
    let objects = Memory::default();
    helper.backup(&objects).await.unwrap();
    let target = layout(&temp.path().join("replacement"));
    let other = target.data.join("orgs/other/account/docs.sqlite3");
    put(&other, b"authoritative other runtime");
    put(&target.repositories.join("unrelated.txt"), b"keep this");
    let helper = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    assert!(helper.restore(&objects).await.is_err());
    assert_eq!(
        std::fs::read(other).unwrap(),
        b"authoritative other runtime"
    );
    assert_eq!(
        std::fs::read(target.repositories.join("unrelated.txt")).unwrap(),
        b"keep this"
    );
    assert!(!target.repositories.join("project").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn helper_reuses_the_launchers_verified_lock_without_releasing_its_ownership() {
    use std::os::fd::AsRawFd;
    let temp = tempfile::tempdir().unwrap();
    let layout = layout(temp.path());
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(layout.data.join(".workspace-runtime.lock"))
        .unwrap();
    file.try_lock().unwrap();
    let helper =
        Recovery::acquire_inherited(binding(), layout.clone(), key(), Some(file.as_raw_fd()))
            .unwrap();
    assert!(crate::DataLock::acquire(&layout.data, None).is_err());
    drop(helper);
    assert!(crate::DataLock::acquire(&layout.data, None).is_err());
    drop(file);
    assert!(crate::DataLock::acquire(&layout.data, None).is_ok());
}

#[tokio::test]
async fn populated_restored_storage_requires_matching_authority_and_installation_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let source = layout(&temp.path().join("source"));
    let db = seed(&source);
    let objects = Memory::default();
    let capture = Recovery::acquire(binding(), source, key()).unwrap();
    let manifest = capture.backup(&objects).await.unwrap();
    drop(capture);
    drop(db);
    let target = layout(&temp.path().join("target"));
    let restore = Recovery::acquire(binding(), target.clone(), key()).unwrap();
    restore.restore(&objects).await.unwrap();
    let original = binding();
    let native = original.rust_storage.as_ref().unwrap();
    let database = target.data.join(&native.database);
    let grant = RestoreEnrollment {
        receipt: RestoreReceipt {
            binding: original.clone(),
            archive_sha256: manifest.archive_sha256,
        },
        generation: 3,
        machine_id: "fresh-machine".into(),
    };
    let mut bootstrap:zuse_protocol::cloud::CloudRuntimeBootstrap=serde_json::from_value(json!({
        "workspaceId":"workspace","zuseAccountId":"account","chatId":"thread","initialSessionId":"session",
        "runtimeCredential":"fresh-credential","runtimeGeneration":3,"gatewayEpoch":4,"runtimeCredentialExpiresAt":9999999999999i64,
        "gatewayUrl":"ws://localhost/gateway","gatewayProtocol":"zuse-rust-rpc-v1","sealedTranscriptKey":"authority-key",
        "providerSandboxId":"fresh-machine","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"original-storage"}
    })).unwrap();
    assert!(
        crate::Storage::prepare(
            &target.data,
            &database,
            &bootstrap,
            "original-storage",
            "new-enrollment"
        )
        .is_err()
    );
    assert!(!target.data.join("rust-runtime-storage.json").exists());
    let mut wrong = grant.clone();
    wrong.receipt.archive_sha256 = "0".repeat(64);
    bootstrap.extra.insert(
        "rustRecoveryRestore".into(),
        serde_json::to_value(wrong).unwrap(),
    );
    assert!(
        crate::Storage::prepare(
            &target.data,
            &database,
            &bootstrap,
            "original-storage",
            "new-enrollment"
        )
        .is_err()
    );
    assert!(!target.data.join("rust-runtime-storage.json").exists());
    let mut wrong = grant.clone();
    wrong.generation = 2;
    bootstrap.extra.insert(
        "rustRecoveryRestore".into(),
        serde_json::to_value(wrong).unwrap(),
    );
    assert!(
        crate::Storage::prepare(
            &target.data,
            &database,
            &bootstrap,
            "original-storage",
            "new-enrollment"
        )
        .is_err()
    );
    let mut wrong = grant.clone();
    wrong.machine_id = "other-machine".into();
    bootstrap.extra.insert(
        "rustRecoveryRestore".into(),
        serde_json::to_value(wrong).unwrap(),
    );
    assert!(
        crate::Storage::prepare(
            &target.data,
            &database,
            &bootstrap,
            "original-storage",
            "new-enrollment"
        )
        .is_err()
    );
    bootstrap.extra.insert(
        "rustRecoveryRestore".into(),
        serde_json::to_value(grant).unwrap(),
    );
    let mut storage = crate::Storage::prepare(
        &target.data,
        &database,
        &bootstrap,
        "original-storage",
        "new-enrollment",
    )
    .unwrap();
    assert!(storage.initialized());
    storage.commit().unwrap();
    assert_eq!(session(&target.data).read_entries().unwrap().len(), 350);
    assert_eq!(
        crate::Storage::enrollment(&target.data).unwrap().as_deref(),
        Some("new-enrollment")
    );
    assert!(
        crate::Storage::prepare(
            &target.data,
            &database,
            &bootstrap,
            "original-storage",
            "different-enrollment"
        )
        .is_err()
    );
}
