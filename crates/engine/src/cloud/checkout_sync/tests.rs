//! A cloud chat's files moving through a real local checkout: what is saved,
//! what changes, what never does, and getting back after an interruption.
use super::*;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for name in repo::REPOSITORY_VARIABLES {
        command.env_remove(name);
    }
    let output = command
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

/// The cloud chat's files, received the way a snapshot arrives: bytes the
/// checkout already has (`have`) are not sent.
async fn snapshot(cloud: &Path, objects: &Path, have: Vec<String>) -> Vec<mirror::ReceivedFile> {
    let stream =
        mirror::snapshot_stream(cloud.to_owned(), "checkout".into(), have.clone()).unwrap();
    mirror::receive_stream(
        "w",
        stream,
        objects,
        have.into_iter().collect(),
        CancellationToken::new(),
    )
    .await
    .unwrap()
}

async fn checkpoint_of(local: &Path, cloud: &Path, parent: &str) -> String {
    let scratch = tempfile::tempdir().unwrap();
    let objects = scratch.path().join("objects");
    let mut known = repo::known_blobs(local, parent).unwrap();
    // Unchanged files (README, .gitignore …) come without bytes.
    assert!(!known.is_empty());
    let files = snapshot(cloud, &objects, known.keys().cloned().collect()).await;
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

    let restored = repo::restore(&local, &saved, true).unwrap();
    assert_eq!(restored, repo::Restored::default());
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert_local_work(&local);
    assert!(read(&local, "src/later.rs").is_none());
    assert!(read(&local, "run.sh").is_none());
    // No hidden refs are left behind.
    assert_eq!(git(&local, &["for-each-ref", "refs/worktree"]), "");
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

    let restored = repo::restore(&local, &saved, true).unwrap();
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
    repo::restore(&local, &saved, true).unwrap();
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
    // The worker puts the folder back from wherever entering stopped.
    repo::restore(&local, &saved, false).unwrap();
    assert_eq!(
        read(&local, "src/new.rs").unwrap(),
        "my own untracked file\n"
    );
    assert_local_work(&local);
    assert_eq!(git(&local, &["for-each-ref", "refs/worktree"]), "");
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
    repo::restore(&local, &saved, true).unwrap();

    let saved = repo::save(&local).unwrap();
    assert_eq!(saved.head, head);
    let second = checkpoint_of(&local, &other, &saved.head).await;
    repo::enter(&local, &saved, &second).unwrap();
    assert!(read(&local, "first.txt").is_none());
    assert_eq!(read(&local, "second.txt").unwrap(), "second chat\n");
    repo::restore(&local, &saved, true).unwrap();
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
    repo::restore(&local, &saved, true).unwrap();
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
        version: VERSION_CHECKOUT,
        mode: Mode::Checkout,
        source: None,
        repository: None,
        url: None,
        migrating: false,
        account_id: "owner".into(),
        workspace_id: "w".into(),
        chat_id: "chat".into(),
        path: local.to_owned(),
        phase,
        branch: saved.branch.clone(),
        head: Some(saved.head.clone()),
        changes: saved.changes.clone(),
        checkpoint: Some(checkpoint.into()),
        pending: None,
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
    assert_eq!(git(&local, &["for-each-ref", "refs/worktree"]), "");
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
        version: VERSION_CHECKOUT,
        mode: Mode::Checkout,
        source: None,
        repository: None,
        url: None,
        migrating: false,
        account_id: "owner".into(),
        workspace_id: "w".into(),
        chat_id: "chat".into(),
        path: PathBuf::from("/somewhere/repo"),
        phase: Phase::On,
        branch: None,
        head: None,
        changes: None,
        checkpoint: None,
        pending: None,
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
fn a_clone_is_found_from_any_folder_inside_it() {
    let (_root, local, _cloud) = repos();
    assert_eq!(repo::clone_root(&local).unwrap(), local);
    assert_eq!(repo::clone_root(&local.join("src")).unwrap(), local);
    assert_eq!(repo::clone_root(local.parent().unwrap()), None);
}

#[tokio::test]
async fn a_branch_made_at_the_checkpoint_pauses_and_resuming_detaches_again() {
    let (_root, local, cloud) = repos();
    let saved = repo::save(&local).unwrap();
    write(&cloud, "src/lib.rs", "cloud\n");
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    repo::enter(&local, &saved, &checkpoint).unwrap();
    assert!(repo::holds(&local, &checkpoint));
    git(&local, &["switch", "-q", "-c", "my-branch"]);
    // Same commit, same files, but no longer the chat's checkout.
    assert!(!repo::holds(&local, &checkpoint));
    assert!(repo::local_edits(&local, &checkpoint).unwrap().is_some());
    repo::return_to(&local, &checkpoint).unwrap();
    assert!(repo::holds(&local, &checkpoint));
    assert_eq!(git(&local, &["rev-parse", "my-branch"]), checkpoint);
}

#[tokio::test]
async fn an_untracked_folder_where_a_tracked_file_was_deleted_is_never_reset_away() {
    let (_root, local, cloud) = repos();
    // The tracked file is deleted (staged) and an untracked folder takes
    // its name.
    git(&local, &["rm", "-q", "src/old.rs"]);
    write(&local, "src/old.rs/local.txt", "mine\n");
    write(&cloud, "x.txt", "x\n");
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    let error = repo::enter(&local, &saved, &checkpoint).unwrap_err();
    assert!(error.to_string().contains("src/old.rs"), "{error}");
    assert_eq!(read(&local, "src/old.rs/local.txt").unwrap(), "mine\n");
    repo::restore(&local, &saved, false).unwrap_err();
    assert_eq!(read(&local, "src/old.rs/local.txt").unwrap(), "mine\n");
    assert!(
        git(&local, &["diff", "--cached", "--name-only"]).contains("src/old.rs"),
        "the staged deletion is still staged"
    );
}

// A chat's folder of its own.

fn worktrees(clone: &Path) -> String {
    git(clone, &["worktree", "list", "--porcelain"])
}

fn folder_state(path: &Path, source: Option<&Path>) -> State {
    State::folder(
        "owner",
        "w",
        "chat",
        path.to_owned(),
        "github.com/example/repo",
        source.map(Path::to_owned),
        None,
    )
}

/// Make the chat's folder and move it to the cloud chat's files, as the
/// worker's first step does. Returns the checkpoint.
async fn first_sync(folder: &Path, cloud: &Path) -> (repo::Saved, String) {
    let saved = repo::save(folder).unwrap();
    let checkpoint = checkpoint_of(folder, cloud, &saved.head).await;
    repo::enter(folder, &saved, &checkpoint).unwrap();
    (saved, checkpoint)
}

#[tokio::test]
async fn a_chat_folder_is_a_worktree_of_the_users_clone_and_never_touches_it() {
    let (root, local, cloud) = repos();
    let head = git(&local, &["rev-parse", "HEAD"]);
    local_work(&local);
    cloud_work(&cloud);
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    let made =
        repo::create_folder(&folder, Some(&local), "https://example.invalid/repo.git").unwrap();
    assert_eq!(made, repo::Made::Worktree);
    assert!(folder.join(".git").is_file(), "a worktree, not a copy");
    assert!(worktrees(&local).contains(folder.to_str().unwrap()));
    assert_eq!(git(&folder, &["rev-parse", "HEAD"]), head);
    // The user's untracked and ignored files never come along.
    assert!(read(&folder, "notes.txt").is_none());
    assert!(read(&folder, ".env").is_none());

    let (_saved, checkpoint) = first_sync(&folder, &cloud).await;
    assert!(repo::holds(&folder, &checkpoint));
    assert_eq!(
        read(&folder, "src/lib.rs").unwrap(),
        "pub fn a() {}\npub fn cloud() {}\n"
    );
    assert_eq!(read(&folder, "src/new.rs").unwrap(), "pub fn fresh() {}\n");
    assert!(read(&folder, "src/old.rs").is_none());
    // The user's own checkout is exactly as it was: branch, commit, staged
    // and unstaged changes, untracked and ignored files.
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert_local_work(&local);
    assert_eq!(git(&local, &["stash", "list"]), "");

    // Later cloud changes keep arriving, in the folder only.
    write(&cloud, "src/later.rs", "later\n");
    let base = git(&folder, &["rev-parse", "HEAD^"]);
    let next = checkpoint_of(&folder, &cloud, &base).await;
    repo::advance(&folder, &checkpoint, &next).unwrap();
    assert_eq!(read(&folder, "src/later.rs").unwrap(), "later\n");
    assert!(read(&local, "src/later.rs").is_none());
    assert_local_work(&local);
}

#[tokio::test]
async fn without_a_clone_the_folder_is_a_fresh_shallow_clone() {
    let (root, _local, cloud) = repos();
    let origin = root.path().join("origin");
    let url = format!("file://{}", origin.display());
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    assert_eq!(
        repo::create_folder(&folder, None, &url).unwrap(),
        repo::Made::Clone
    );
    assert!(folder.join(".git").is_dir());
    assert_eq!(
        git(&folder, &["rev-parse", "HEAD"]),
        git(&origin, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        git(&folder, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    write(&cloud, "cloud.txt", "cloud\n");
    let (_saved, checkpoint) = first_sync(&folder, &cloud).await;
    assert!(repo::holds(&folder, &checkpoint));
    assert_eq!(read(&folder, "cloud.txt").unwrap(), "cloud\n");
}

#[tokio::test]
async fn when_cloning_fails_every_file_arrives_with_the_first_snapshot() {
    let (root, _local, cloud) = repos();
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    assert_eq!(
        repo::create_folder(&folder, None, "file:///nowhere/at/all.git").unwrap(),
        repo::Made::Empty
    );
    let base = repo::head(&folder).unwrap();
    assert!(repo::known_blobs(&folder, &base).unwrap().is_empty());
    let scratch = tempfile::tempdir().unwrap();
    let objects = scratch.path().join("objects");
    let files = snapshot(&cloud, &objects, vec![]).await;
    let mut known = HashMap::new();
    let checkpoint =
        repo::checkpoint(&folder, &files, &objects, &mut known, &base, "cloud").unwrap();
    let saved = repo::save(&folder).unwrap();
    repo::enter(&folder, &saved, &checkpoint).unwrap();
    assert_eq!(read(&folder, "README.md").unwrap(), "hello\n");
    assert_eq!(read(&folder, "src/lib.rs").unwrap(), "pub fn a() {}\n");
}

#[test]
fn a_folder_that_isnt_ours_is_never_used_or_removed() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("taken");
    write(&folder, "mine.txt", "mine\n");
    assert!(repo::create_folder(&folder, None, "file:///nowhere.git").is_err());
    assert!(repo::remove_folder(&folder).is_err());
    assert_eq!(read(&folder, "mine.txt").unwrap(), "mine\n");
}

#[tokio::test]
async fn edits_in_the_chat_folder_pause_sync_and_discarding_resumes_without_the_users_stash() {
    let (root, local, cloud) = repos();
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    repo::create_folder(&folder, Some(&local), "https://example.invalid/r.git").unwrap();
    write(&cloud, "src/lib.rs", "cloud\n");
    let (saved, checkpoint) = first_sync(&folder, &cloud).await;
    write(&folder, "src/lib.rs", "edited in the chat's folder\n");
    assert!(repo::local_edits(&folder, &checkpoint).unwrap().is_some());
    write(&cloud, "src/lib.rs", "cloud again\n");
    let next = checkpoint_of(&folder, &cloud, &saved.head).await;
    // Never overwritten while paused.
    assert!(repo::advance(&folder, &checkpoint, &next).is_err());
    assert_eq!(
        read(&folder, "src/lib.rs").unwrap(),
        "edited in the chat's folder\n"
    );
    // "Discard my edits and resume".
    repo::return_to(&folder, &checkpoint).unwrap();
    assert_eq!(repo::local_edits(&folder, &checkpoint).unwrap(), None);
    repo::advance(&folder, &checkpoint, &next).unwrap();
    assert_eq!(read(&folder, "src/lib.rs").unwrap(), "cloud again\n");
    // The clone's stash list (shared by its worktrees) stays the user's.
    assert_eq!(git(&local, &["stash", "list"]), "");
}

#[tokio::test]
async fn turning_sync_off_keeps_the_folder_and_turning_it_on_reuses_it() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    repo::create_folder(&folder, Some(&local), "https://example.invalid/r.git").unwrap();
    write(&cloud, "cloud.txt", "cloud\n");
    let (saved, checkpoint) = first_sync(&folder, &cloud).await;
    let mut state = folder_state(&folder, Some(&local));
    state.head = Some(saved.head.clone());
    state.checkpoint = Some(checkpoint.clone());
    write_state(&state_file(&profile, &folder), &state).unwrap();
    // While on, the chat's commands on this computer start in its folder.
    assert_eq!(
        synced_path(&profile, "owner", "w", "chat").as_deref(),
        Some(folder.as_path())
    );

    let manager = Manager::default();
    manager.hydrate(&profile);
    let status = manager.end(&folder, None).await.unwrap();
    assert!(!status.enabled);
    assert_eq!(status.state, "off");
    assert_eq!(status.local_path.as_deref(), Some(folder.as_path()));
    assert_eq!(read(&folder, "cloud.txt").unwrap(), "cloud\n");
    assert!(worktrees(&local).contains(folder.to_str().unwrap()));
    assert_eq!(synced_path(&profile, "owner", "w", "chat"), None);
    // Remembered after a restart, still off, folder still there.
    let again = Manager::default();
    again.hydrate(&profile);
    let status = again.status("owner", "w", "chat");
    assert_eq!((status.enabled, status.state), (false, "off"), "{status:?}");
    assert_eq!(status.local_path.as_deref(), Some(folder.as_path()));
    assert_eq!(
        again.by_chat("owner", "w", "chat").as_deref(),
        Some(folder.as_path())
    );
}

#[tokio::test]
async fn archiving_the_chat_removes_its_folder_unless_it_was_changed_here() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    write(&cloud, "cloud.txt", "cloud\n");
    for edited in [false, true] {
        let folder = root
            .path()
            .join(format!("zuse/cloud/example-repo/chat-{edited}"));
        repo::create_folder(&folder, Some(&local), "https://example.invalid/r.git").unwrap();
        let (saved, checkpoint) = first_sync(&folder, &cloud).await;
        // Ignored build output never keeps a folder.
        write(&folder, "target/debug/app", "build output\n");
        if edited {
            write(&folder, "mine.txt", "a new file made here\n");
        }
        let mut state = folder_state(&folder, Some(&local));
        state.chat_id = format!("chat-{edited}");
        state.head = Some(saved.head.clone());
        state.checkpoint = Some(checkpoint.clone());
        let file = state_file(&profile, &folder);
        write_state(&file, &state).unwrap();
        let manager = Manager::default();
        manager.hydrate(&profile);
        manager.archived(&folder).await.unwrap();
        if edited {
            assert_eq!(read(&folder, "mine.txt").unwrap(), "a new file made here\n");
            let status = manager.status("owner", "w", "chat-true");
            assert_eq!(status.state, "off");
            assert!(status.notice.unwrap().contains("stays at"));
            assert!(file.exists());
        } else {
            assert!(!folder.exists());
            assert!(!worktrees(&local).contains(folder.to_str().unwrap()));
            assert!(!file.exists());
            assert_eq!(manager.status("owner", "w", "chat-false").local_path, None);
        }
    }
    // The user's clone is untouched either way.
    assert_eq!(git(&local, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert!(git(&local, &["status", "--porcelain"]).is_empty());
}

#[tokio::test]
async fn deleting_the_folder_removes_it_and_its_worktree_entry_only() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    let folder = root.path().join("zuse/cloud/example-repo/chat");
    repo::create_folder(&folder, Some(&local), "https://example.invalid/r.git").unwrap();
    let (saved, checkpoint) = first_sync(&folder, &cloud).await;
    // A branch made in the folder lives in the user's clone and survives.
    git(&folder, &["branch", "kept-branch"]);
    let mut state = folder_state(&folder, Some(&local));
    state.head = Some(saved.head);
    state.checkpoint = Some(checkpoint);
    state.phase = Phase::Stopped;
    write_state(&state_file(&profile, &folder), &state).unwrap();
    let manager = Manager::default();
    manager.hydrate(&profile);
    let status = manager.delete(&folder).await.unwrap();
    assert_eq!((status.state, status.local_path), ("off", None));
    assert!(!folder.exists());
    assert!(
        !folder.parent().unwrap().exists(),
        "its empty group goes too"
    );
    assert!(!worktrees(&local).contains(folder.to_str().unwrap()));
    assert!(git(&local, &["branch", "--list", "kept-branch"]).contains("kept-branch"));
    assert!(read(&local, "README.md").is_some());
    assert_eq!(manager.by_chat("owner", "w", "chat"), None);
}

#[tokio::test]
async fn an_earlier_sync_of_the_users_checkout_goes_back_exactly_then_moves_to_a_folder() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    let folders = root.path().join("zuse/cloud");
    let head = git(&local, &["rev-parse", "HEAD"]);
    local_work(&local);
    write(&cloud, "cloud.txt", "cloud\n");
    // As an earlier version left it: the user's checkout on a checkpoint.
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    let legacy = state_file(&profile, &local);
    write_state(
        &legacy,
        &record_state(&local, &saved, &checkpoint, Phase::On),
    )
    .unwrap();
    repo::enter(&local, &saved, &checkpoint).unwrap();
    assert_eq!(read(&local, "cloud.txt").unwrap(), "cloud\n");

    let manager = Manager::default();
    manager.hydrate(&profile);
    let folder = manager.migrate(&profile, &folders, &local).await.unwrap();
    // The checkout is exactly as the user left it.
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), head);
    assert_local_work(&local);
    assert!(read(&local, "cloud.txt").is_none());
    assert_eq!(git(&local, &["for-each-ref", "refs/worktree"]), "");
    assert!(!legacy.exists());
    // The chat now has a folder of its own, made from that checkout.
    assert!(folder.starts_with(folders.canonicalize().unwrap()));
    let state = manager.state(&folder).unwrap();
    assert_eq!(
        (state.mode, state.phase, state.version),
        (Mode::Folder, Phase::On, VERSION_FOLDER)
    );
    assert_eq!(state.source.as_deref(), Some(local.as_path()));
    assert_eq!(
        manager.by_chat("owner", "w", "chat").as_deref(),
        Some(folder.as_path())
    );
    let status = manager.status("owner", "w", "chat");
    assert!(
        status
            .notice
            .unwrap()
            .contains("went back to how you left it")
    );
    let loaded = read_states(&profile);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1.mode, Mode::Folder);
    // The connector reports no folder until the new one holds the chat's
    // files, and never the user's own checkout again.
    assert_eq!(synced_path(&profile, "owner", "w", "chat"), None);
    // The worker's first step makes it from the checkout and syncs it.
    repo::create_folder(
        &folder,
        state.source.as_deref(),
        "https://example.invalid/r.git",
    )
    .unwrap();
    let (saved, checkpoint) = first_sync(&folder, &cloud).await;
    let mut on = state.clone();
    on.head = Some(saved.head);
    on.checkpoint = Some(checkpoint);
    write_state(&state_file(&profile, &folder), &on).unwrap();
    assert_eq!(
        synced_path(&profile, "owner", "w", "chat").as_deref(),
        Some(folder.as_path())
    );
    assert_eq!(read(&folder, "cloud.txt").unwrap(), "cloud\n");
    assert_local_work(&local);
}

#[tokio::test]
async fn a_move_interrupted_while_putting_the_checkout_back_finishes_after_a_restart() {
    let (root, local, cloud) = repos();
    let profile = root.path().join("profile");
    let folders = root.path().join("zuse/cloud");
    local_work(&local);
    write(&cloud, "cloud.txt", "cloud\n");
    let saved = repo::save(&local).unwrap();
    let checkpoint = checkpoint_of(&local, &cloud, &saved.head).await;
    let mut state = record_state(&local, &saved, &checkpoint, Phase::Restoring);
    state.migrating = true;
    write_state(&state_file(&profile, &local), &state).unwrap();
    repo::enter(&local, &saved, &checkpoint).unwrap();
    // Half way: tracked files reset, then the app stopped.
    git(&local, &["reset", "-q", "--hard", "HEAD"]);
    let manager = Manager::default();
    manager.hydrate(&profile);
    assert_eq!(manager.status("owner", "w", "chat").state, "restoring");
    let folder = manager.migrate(&profile, &folders, &local).await.unwrap();
    assert_local_work(&local);
    assert_eq!(manager.state(&folder).unwrap().mode, Mode::Folder);
}

#[test]
fn each_chat_gets_its_own_folder_named_after_its_repository() {
    let root = Path::new("/home/me/zuse/cloud");
    let none = |_: &Path| false;
    assert_eq!(
        folder_for(
            root,
            "github.com/OpcodeHQ/Zuse",
            "ses_01JXA7Q2M9KD8F3K2C1A",
            &none
        ),
        root.join("opcodehq-zuse/8f3k2c1a")
    );
    assert_eq!(folder_for(root, "repo", "a/b", &none), root.join("repo/ab"));
    let first = root.join("opcodehq-zuse/8f3k2c1a");
    let taken = |path: &Path| path == first;
    assert_eq!(
        folder_for(root, "github.com/opcodehq/zuse", "x8f3k2c1a", &taken),
        root.join("opcodehq-zuse/8f3k2c1a-2")
    );
}
