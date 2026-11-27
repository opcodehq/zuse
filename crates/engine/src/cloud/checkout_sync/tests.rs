//! A cloud chat's files moving through a real local checkout: what is saved,
//! what changes, what never does, and getting back after an interruption.
use super::*;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(dir: &Path, path: &str) -> Option<String> {
    fs::read_to_string(dir.join(path)).ok()
}

/// A repository with a pushed-looking history, cloned as the user's
/// checkout and as the cloud chat's checkout.
fn repos() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    write(&origin, "README.md", "hello\n");
    write(&origin, "src/lib.rs", "pub fn a() {}\n");
    write(&origin, "src/old.rs", "old\n");
    write(&origin, ".gitignore", "target/\n.env\n");
    git(&origin, &["add", "-A"]);
    git(&origin, &["commit", "-q", "-m", "base"]);
    let local = root.path().join("local");
    let cloud = root.path().join("cloud");
    for dir in [&local, &cloud] {
        git(
            root.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                dir.to_str().unwrap(),
            ],
        );
    }
    let local = local.canonicalize().unwrap();
    (root, local, cloud)
}

/// The cloud chat's files, received the way a snapshot arrives.
async fn snapshot(cloud: &Path, objects: &Path) -> Vec<mirror::ReceivedFile> {
    let stream = mirror::snapshot_stream(cloud.to_owned(), "checkout".into(), vec![]).unwrap();
    mirror::receive_stream("w", stream, objects, CancellationToken::new())
        .await
        .unwrap()
}

async fn checkpoint_of(local: &Path, cloud: &Path, parent: &str) -> String {
    let scratch = tempfile::tempdir().unwrap();
    let objects = scratch.path().join("objects");
    let files = snapshot(cloud, &objects).await;
    let mut known = repo::known_blobs(local, parent).unwrap();
    repo::checkpoint(local, &files, &objects, &mut known, parent, "cloud").unwrap()
}

/// Local work in progress of every kind git tracks, plus files it doesn't.
fn local_work(local: &Path) {
    write(local, "README.md", "hello\nlocal edit\n");
    write(local, "staged.txt", "staged new file\n");
    git(local, &["add", "staged.txt"]);
    write(local, "src/lib.rs", "pub fn a() {}\n// staged\n");
    git(local, &["add", "src/lib.rs"]);
    write(
        local,
        "src/lib.rs",
        "pub fn a() {}\n// staged\n// and unstaged\n",
    );
    fs::remove_file(local.join("src/old.rs")).unwrap();
    write(local, "notes.txt", "untracked\n");
    write(local, "target/debug/build.log", "ignored build output\n");
    write(local, ".env", "SECRET=1\n");
}

fn assert_local_work(local: &Path) {
    assert_eq!(read(local, "README.md").unwrap(), "hello\nlocal edit\n");
    assert_eq!(read(local, "staged.txt").unwrap(), "staged new file\n");
    assert_eq!(
        read(local, "src/lib.rs").unwrap(),
        "pub fn a() {}\n// staged\n// and unstaged\n"
    );
    assert!(read(local, "src/old.rs").is_none());
    assert_eq!(read(local, "notes.txt").unwrap(), "untracked\n");
    assert_eq!(
        read(local, "target/debug/build.log").unwrap(),
        "ignored build output\n"
    );
    assert_eq!(read(local, ".env").unwrap(), "SECRET=1\n");
    // Staged stays staged, unstaged stays unstaged.
    let staged = git(local, &["diff", "--cached", "--name-only"]);
    assert!(staged.contains("staged.txt") && staged.contains("src/lib.rs"));
    assert!(git(local, &["diff", "--name-only"]).contains("README.md"));
    assert_eq!(git(local, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

/// The cloud chat edited, added, deleted, made executable and linked files,
/// some committed, some only in its working tree.
fn cloud_work(cloud: &Path) {
    write(cloud, "src/lib.rs", "pub fn a() {}\npub fn cloud() {}\n");
    git(cloud, &["commit", "-qam", "cloud commit"]);
    write(cloud, "src/new.rs", "pub fn fresh() {}\n");
    write(cloud, "run.sh", "#!/bin/sh\necho hi\n");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(cloud.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("src/lib.rs", cloud.join("lib-link")).unwrap();
    fs::remove_file(cloud.join("src/old.rs")).unwrap();
    write(cloud, "README.md", "hello from the cloud\n");
    // Cloud build output stays in the cloud.
    write(cloud, "target/release/app", "binary\n");
    write(cloud, ".env", "CLOUD=1\n");
}

#[tokio::test]
async fn sync_round_trip_keeps_local_work_and_never_touches_untracked_files() {
    let (_root, local, cloud) = repos();
    let head = git(&local, &["rev-parse", "HEAD"]);
    local_work(&local);
    cloud_work(&cloud);
    let saved = repo::save(&local).unwrap();
    assert_eq!(saved.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(saved.head, head);
    assert!(saved.changes.is_some());
    // Saving changes nothing in the folder.
    assert_local_work(&local);

    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &checkpoint).unwrap();
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), checkpoint);
    assert_eq!(
        read(&local, "src/lib.rs").unwrap(),
        "pub fn a() {}\npub fn cloud() {}\n"
    );
    assert_eq!(read(&local, "README.md").unwrap(), "hello from the cloud\n");
    assert_eq!(read(&local, "src/new.rs").unwrap(), "pub fn fresh() {}\n");
    assert!(read(&local, "src/old.rs").is_none());
    assert!(read(&local, "staged.txt").is_none());
    use std::os::unix::fs::PermissionsExt;
    assert_ne!(
        fs::metadata(local.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert_eq!(
        fs::read_link(local.join("lib-link")).unwrap(),
        PathBuf::from("src/lib.rs")
    );
    // Untracked and ignored local files are exactly as they were, and the
    // cloud's ignored files never arrive.
    assert_eq!(read(&local, "notes.txt").unwrap(), "untracked\n");
    assert_eq!(read(&local, ".env").unwrap(), "SECRET=1\n");
    assert_eq!(
        read(&local, "target/debug/build.log").unwrap(),
        "ignored build output\n"
    );
    assert!(read(&local, "target/release/app").is_none());
    assert!(!repo::has_tracked_changes(&local).unwrap());
    assert_eq!(repo::local_edits(&local, &checkpoint).unwrap(), None);

    // The next cloud change moves the checkout again: a new file and a
    // deletion.
    write(&cloud, "src/later.rs", "later\n");
    fs::remove_file(cloud.join("src/new.rs")).unwrap();
    let next = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::advance(&local, &checkpoint, &next).unwrap();
    assert_eq!(read(&local, "src/later.rs").unwrap(), "later\n");
    assert!(read(&local, "src/new.rs").is_none());
    assert_eq!(read(&local, "notes.txt").unwrap(), "untracked\n");

    let restored = repo::restore(&local, &saved).unwrap();
    assert_eq!(restored, repo::Restored::default());
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert_local_work(&local);
    assert!(read(&local, "src/later.rs").is_none());
    assert!(read(&local, "run.sh").is_none());
    // No hidden refs are left behind.
    assert_eq!(git(&local, &["for-each-ref", "refs/zuse"]), "");
    assert_eq!(git(&local, &["stash", "list"]), "");
}

#[tokio::test]
async fn edits_made_in_the_folder_pause_sync_and_are_kept_on_restore() {
    let (_root, local, cloud) = repos();
    let saved = repo::save(&local).unwrap();
    assert_eq!(saved.changes, None);
    write(&cloud, "src/lib.rs", "cloud\n");
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &checkpoint).unwrap();

    write(&local, "src/lib.rs", "edited on this computer\n");
    assert!(repo::local_edits(&local, &checkpoint).unwrap().is_some());
    write(&cloud, "src/lib.rs", "cloud again\n");
    let next = checkpoint_of(&local, &cloud, &saved.head).await;
    // Never overwritten.
    assert!(repo::advance(&local, &checkpoint, &next).is_err());
    assert_eq!(
        read(&local, "src/lib.rs").unwrap(),
        "edited on this computer\n"
    );

    let restored = repo::restore(&local, &saved).unwrap();
    assert!(restored.kept_edits);
    assert_eq!(read(&local, "src/lib.rs").unwrap(), "pub fn a() {}\n");
    assert!(git(&local, &["stash", "list"]).contains("edits made while synced"));
    // The kept edit is intact (on top of the cloud checkpoint it was made on).
    assert_eq!(
        git(&local, &["show", "stash@{0}:src/lib.rs"]),
        "edited on this computer"
    );
}

#[tokio::test]
async fn resuming_keeps_edits_and_returns_to_the_checkpoint() {
    let (_root, local, cloud) = repos();
    let saved = repo::save(&local).unwrap();
    write(&cloud, "src/lib.rs", "cloud\n");
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &checkpoint).unwrap();
    git(&local, &["checkout", "-q", "main"]);
    assert!(repo::local_edits(&local, &checkpoint).unwrap().is_some());
    write(&local, "README.md", "edit on main\n");
    assert!(repo::keep_in_stash_list(&local, "kept").unwrap());
    repo::return_to(&local, &checkpoint).unwrap();
    assert_eq!(repo::local_edits(&local, &checkpoint).unwrap(), None);
    assert_eq!(read(&local, "src/lib.rs").unwrap(), "cloud\n");
    repo::restore(&local, &saved).unwrap();
    assert_eq!(git(&local, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(read(&local, "README.md").unwrap(), "hello\n");
}

#[tokio::test]
async fn an_untracked_file_in_the_way_stops_sync_without_losing_anything() {
    let (_root, local, cloud) = repos();
    local_work(&local);
    write(&local, "src/new.rs", "my own untracked file\n");
    write(&cloud, "src/new.rs", "cloud file\n");
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    let error = repo::enter(&local, &saved, &checkpoint).unwrap_err();
    assert!(error.to_string().contains("src/new.rs"), "{error}");
    assert_eq!(
        read(&local, "src/new.rs").unwrap(),
        "my own untracked file\n"
    );
    assert_local_work(&local);
    assert_eq!(git(&local, &["for-each-ref", "refs/zuse"]), "");
}

#[tokio::test]
async fn switching_chats_replaces_the_checkpoint_and_still_restores_the_original() {
    let (_root, local, cloud) = repos();
    let other = local.parent().unwrap().join("cloud-two");
    git(
        local.parent().unwrap(),
        &[
            "clone",
            "-q",
            local.parent().unwrap().join("origin").to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    let head = git(&local, &["rev-parse", "HEAD"]);
    local_work(&local);
    write(&cloud, "first.txt", "first chat\n");
    write(&other, "second.txt", "second chat\n");

    let saved = repo::save(&local).unwrap();
    let first = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &first).unwrap();
    assert_eq!(read(&local, "first.txt").unwrap(), "first chat\n");
    repo::restore(&local, &saved).unwrap();

    let saved = repo::save(&local).unwrap();
    assert_eq!(saved.head, head);
    let second = checkpoint_of(&local, &other, &saved.head).await;
    repo::enter(&local, &saved, &second).unwrap();
    assert!(read(&local, "first.txt").is_none());
    assert_eq!(read(&local, "second.txt").unwrap(), "second chat\n");
    repo::restore(&local, &saved).unwrap();
    assert_local_work(&local);
}

#[tokio::test]
async fn a_detached_checkout_and_submodule_pointers_come_back_as_they_were() {
    let (_root, local, cloud) = repos();
    let head = git(&local, &["rev-parse", "HEAD"]);
    git(&local, &["checkout", "-q", "--detach"]);
    let saved = repo::save(&local).unwrap();
    assert_eq!(saved.branch, None);
    write(&cloud, "x.txt", "x\n");
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &checkpoint).unwrap();
    repo::restore(&local, &saved).unwrap();
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&local)
            .args(["symbolic-ref", "-q", "HEAD"])
            .status()
            .unwrap()
            .code()
            == Some(1)
    );
}

fn record_state(local: &Path, saved: &repo::Saved, checkpoint: &str, phase: Phase) -> State {
    State {
        version: 1,
        account_id: "owner".into(),
        workspace_id: "w".into(),
        chat_id: "chat".into(),
        path: local.to_owned(),
        phase,
        branch: saved.branch.clone(),
        head: Some(saved.head.clone()),
        changes: saved.changes.clone(),
        checkpoint: Some(checkpoint.into()),
    }
}

#[tokio::test]
async fn an_interrupted_restore_finishes_after_a_restart() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    local_work(&local);
    write(&cloud, "cloud.txt", "cloud\n");
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    let file = state_file(&profile, &local);
    write_state(&file, &record_state(&local, &saved, &checkpoint, Phase::On)).unwrap();
    repo::enter(&local, &saved, &checkpoint).unwrap();
    // The folder reports as synced to that chat, so its commands start there.
    assert_eq!(
        synced_path(&profile, "owner", "w", "chat").as_deref(),
        Some(local.as_path())
    );
    assert_eq!(synced_path(&profile, "intruder", "w", "chat"), None);
    // The app stopped while putting it back: the restore began, the
    // checkout is half way (tracked files reset), nothing more was recorded.
    write_state(
        &file,
        &record_state(&local, &saved, &checkpoint, Phase::Restoring),
    )
    .unwrap();
    git(&local, &["reset", "-q", "--hard", "HEAD"]);
    assert_eq!(synced_path(&profile, "owner", "w", "chat"), None);

    // A new engine finds it and finishes.
    let manager = Manager::default();
    manager.hydrate(&profile);
    assert_eq!(manager.status("owner", "w", "chat").state, "restoring");
    let status = manager.restore(&local, None).await.unwrap();
    assert_eq!(status.state, "off");
    assert_eq!(
        git(&local, &["rev-parse", "HEAD"]),
        saved.head,
        "back on the saved commit"
    );
    assert_local_work(&local);
    assert!(read(&local, "cloud.txt").is_none());
    assert!(!file.exists());
    assert_eq!(git(&local, &["for-each-ref", "refs/zuse"]), "");
}

#[tokio::test]
async fn a_restart_while_synced_restores_from_the_persisted_state_alone() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    let head = git(&local, &["rev-parse", "HEAD"]);
    local_work(&local);
    write(&cloud, "cloud.txt", "cloud\n");
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    write_state(
        &state_file(&profile, &local),
        &record_state(&local, &saved, &checkpoint, Phase::On),
    )
    .unwrap();
    repo::enter(&local, &saved, &checkpoint).unwrap();
    // Everything else is gone: only the profile's file and the checkout's
    // hidden refs remember how it was.
    let manager = Manager::default();
    manager.hydrate(&profile);
    let status = manager
        .end(&local, Some("signed out".into()))
        .await
        .unwrap();
    assert_eq!(status.notice.as_deref(), Some("signed out"));
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert_local_work(&local);
}

#[test]
fn only_well_formed_states_in_their_own_file_are_loaded() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path();
    let state = State {
        version: 1,
        account_id: "owner".into(),
        workspace_id: "w".into(),
        chat_id: "chat".into(),
        path: PathBuf::from("/somewhere/repo"),
        phase: Phase::On,
        branch: None,
        head: None,
        changes: None,
        checkpoint: None,
    };
    write_state(&state_file(profile, &state.path), &state).unwrap();
    // A file whose name doesn't match its folder is ignored.
    let mut moved = state.clone();
    moved.path = PathBuf::from("/elsewhere");
    write_state(&directory(profile).join("misnamed.json"), &moved).unwrap();
    let loaded = read_states(profile);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, state);
    // Not synced until it holds a checkpoint.
    assert_eq!(synced_path(profile, "owner", "w", "chat"), None);
}

#[test]
fn repository_spellings_match() {
    for spelling in [
        "github.com/OpcodeHQ/Zuse",
        "https://github.com/opcodehq/zuse.git",
        "git@github.com:opcodehq/zuse.git",
        "ssh://git@github.com/opcodehq/zuse",
    ] {
        assert_eq!(
            normalize(spelling),
            "github.com/opcodehq/zuse",
            "{spelling}"
        );
    }
}

#[test]
fn only_a_repository_top_folder_without_an_operation_in_progress_is_accepted() {
    let (_root, local, _cloud) = repos();
    assert_eq!(repo::checkout_root(&local).unwrap(), local);
    assert!(repo::checkout_root(&local.join("src")).is_err());
    fs::write(local.join(".git/MERGE_HEAD"), "x").unwrap();
    assert!(
        repo::checkout_root(&local)
            .unwrap_err()
            .to_string()
            .contains("merge")
    );
}
