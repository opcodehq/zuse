//! Git operations on a synced folder: make a chat's folder (a worktree of
//! the user's clone, or a clone), save how a folder is, move it to a cloud
//! chat's checkpoint, keep it there, put an earlier version's sync of the
//! user's own checkout back exactly, and remove a chat's folder. Untracked
//! and ignored files are never written, moved or removed: every tree change
//! goes through `git read-tree -m -u`, which refuses to overwrite them.
use crate::cloud::mirror::ReceivedFile;
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Hidden refs that keep the saved state (and the current checkpoint)
/// reachable, so `git gc` can never collect them while synced. They are
/// per worktree, so linked worktrees of one repository sync independently.
pub(crate) const REF_HEAD: &str = "refs/worktree/zuse/checkout-sync/head";
pub(crate) const REF_CHANGES: &str = "refs/worktree/zuse/checkout-sync/changes";
pub(crate) const REF_CHECKPOINT: &str = "refs/worktree/zuse/checkout-sync/checkpoint";
/// Variables that would point git at another repository than `-C` names.
pub(crate) const REPOSITORY_VARIABLES: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];
/// The local blobs hashed to recognize files the cloud doesn't need to send.
const SEED_MAX_BLOBS: usize = 100_000;
const SEED_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// How the checkout was before syncing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    /// `refs/heads/…` when a branch was checked out; `None` when detached.
    pub branch: Option<String>,
    pub head: String,
    /// A stash-style commit of staged and unstaged tracked changes, when any.
    pub changes: Option<String>,
}

fn git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.quotePath=false",
            "-c",
            "advice.detachedHead=false",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    for name in REPOSITORY_VARIABLES {
        command.env_remove(name);
    }
    command
}

fn run(mut command: Command) -> Result<String> {
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Couldn't run git. Install the command line tools and try again.")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if message.is_empty() {
                "git failed".into()
            } else {
                message
            }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned())
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = git(dir);
    command.args(args);
    run(command)
}

/// The commit `rev` names, if it does.
fn commit(dir: &Path, rev: &str) -> Result<Option<String>> {
    let output = git(dir)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .output()?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|oid| !oid.is_empty()))
}

pub(crate) fn head(dir: &Path) -> Result<String> {
    commit(dir, "HEAD")?.context("This folder has no commits yet.")
}

/// The `host/owner/repo` of the checkout's `origin` (else its first remote).
pub(crate) fn remote(dir: &Path) -> Option<String> {
    let url = git_ok(dir, &["remote", "get-url", "origin"])
        .ok()
        .or_else(|| {
            let names = git_ok(dir, &["remote"]).ok()?;
            let first = names.lines().next()?.trim().to_owned();
            git_ok(dir, &["remote", "get-url", &first]).ok()
        })?;
    let remote = crate::source_control::parse_git_remote(&url)?;
    Some(format!(
        "{}/{}/{}",
        remote.host, remote.owner, remote.repository
    ))
}

/// Whether tracked files differ from HEAD (staged or not). Untracked and
/// ignored files don't count.
pub(crate) fn has_tracked_changes(dir: &Path) -> Result<bool> {
    let _ = git(dir)
        .args(["update-index", "-q", "--refresh"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(!git_ok(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--ignore-submodules=all",
        ],
    )?
    .is_empty())
}

/// Record the checkout's branch, HEAD and tracked changes, without changing
/// any file, and pin them with hidden refs.
pub(crate) fn save(dir: &Path) -> Result<Saved> {
    let head = head(dir)?;
    let branch = git(dir)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|b| b.starts_with("refs/heads/"));
    let changes = stash_create(dir, "Zuse: your changes before syncing a cloud chat")?;
    pin(dir, REF_HEAD, &head)?;
    if let Some(changes) = &changes {
        pin(dir, REF_CHANGES, changes)?;
    }
    Ok(Saved {
        branch,
        head,
        changes,
    })
}

fn stash_create(dir: &Path, message: &str) -> Result<Option<String>> {
    let _ = git(dir)
        .args(["update-index", "-q", "--refresh"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let oid = git_ok(dir, &["stash", "create", message])?;
    Ok((!oid.is_empty()).then_some(oid))
}

fn pin(dir: &Path, name: &str, oid: &str) -> Result<()> {
    git_ok(
        dir,
        &["update-ref", "-m", "Zuse cloud chat sync", name, oid],
    )
    .map(|_| ())
}

fn unpin(dir: &Path) {
    for name in [REF_HEAD, REF_CHANGES, REF_CHECKPOINT] {
        let _ = git_ok(dir, &["update-ref", "-d", name]);
    }
}

/// Keep the checkout's tracked changes as an entry in its stash list
/// (leaving the files as they are). Returns whether there were any.
pub(crate) fn keep_in_stash_list(dir: &Path, message: &str) -> Result<bool> {
    match stash_create(dir, message)? {
        Some(oid) => {
            git_ok(dir, &["stash", "store", "-m", message, &oid])?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Git blob IDs of `commit`'s files by the SHA-256 of their contents, so a
/// snapshot only transfers files this checkout doesn't already have.
pub(crate) fn known_blobs(dir: &Path, rev: &str) -> Result<HashMap<String, String>> {
    let listing = git_ok(dir, &["ls-tree", "-r", "-z", "--full-tree", "-l", rev])?;
    let mut oids = Vec::new();
    let mut seen = HashSet::new();
    let mut bytes = 0u64;
    for line in listing.split('\0').filter(|l| !l.is_empty()) {
        let Some((meta, _path)) = line.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let (Some(_mode), Some(kind), Some(oid), Some(size)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if kind != "blob" || !seen.insert(oid.to_owned()) {
            continue;
        }
        bytes += size.parse::<u64>().unwrap_or(0);
        if oids.len() >= SEED_MAX_BLOBS || bytes > SEED_MAX_BYTES {
            break;
        }
        oids.push(oid.to_owned());
    }
    let mut known = HashMap::new();
    if oids.is_empty() {
        return Ok(known);
    }
    let mut child = git(dir)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().context("git cat-file")?;
    let writer = std::thread::spawn({
        let oids = oids.clone();
        move || -> std::io::Result<()> {
            for oid in oids {
                writeln!(input, "{oid}")?;
            }
            Ok(())
        }
    });
    let mut output = BufReader::new(child.stdout.take().context("git cat-file")?);
    let mut header = String::new();
    let mut buffer = vec![0; 128 * 1024];
    for _ in 0..oids.len() {
        header.clear();
        if output.read_line(&mut header)? == 0 {
            break;
        }
        let mut parts = header.split_whitespace();
        let (Some(oid), Some(_kind), Some(size)) = (parts.next(), parts.next(), parts.next())
        else {
            break;
        };
        let mut left = size.parse::<u64>().context("git cat-file size")?;
        let mut hash = Sha256::new();
        while left > 0 {
            let want = buffer.len().min(left as usize);
            output.read_exact(&mut buffer[..want])?;
            hash.update(&buffer[..want]);
            left -= want as u64;
        }
        let mut newline = [0u8; 1];
        output.read_exact(&mut newline)?;
        known.insert(format!("{:x}", hash.finalize()), oid.to_owned());
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(known)
}

/// Submodule pointers in `rev`, by path. They stay as the checkout had them.
fn gitlinks(dir: &Path, rev: &str) -> Result<Vec<(String, String)>> {
    let listing = git_ok(dir, &["ls-tree", "-r", "-z", "--full-tree", rev])?;
    Ok(listing
        .split('\0')
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut parts = meta.split_whitespace();
            (parts.next()? == "160000" && parts.next()? == "commit")
                .then(|| (path.to_owned(), parts.next().unwrap_or_default().to_owned()))
        })
        .collect())
}

/// Write a snapshot as a commit on top of `parent`: the cloud chat's files
/// (bytes from `objects`, or blobs `known` already maps), with `parent`'s
/// submodule pointers. `known` learns every blob written.
pub(crate) fn checkpoint(
    dir: &Path,
    files: &[ReceivedFile],
    objects: &Path,
    known: &mut HashMap<String, String>,
    parent: &str,
    message: &str,
) -> Result<String> {
    let links = gitlinks(dir, parent)?;
    let inside_submodule = |path: &str| {
        links
            .iter()
            .any(|(link, _)| path == link || path.starts_with(&format!("{link}/")))
    };
    let staging = tempfile::tempdir_in(objects.parent().unwrap_or(objects))?;
    // Bytes git hasn't seen yet become blobs in one pass.
    let mut missing = Vec::new();
    let mut queued = HashSet::new();
    for file in files {
        if known.contains_key(&file.hash) || !queued.insert(file.hash.clone()) {
            continue;
        }
        let source = match &file.link {
            Some(target) => {
                let path = staging.path().join(format!("link-{}", file.hash));
                std::fs::write(&path, target.as_bytes())?;
                path
            }
            None => objects.join(&file.hash),
        };
        ensure!(
            source.is_file(),
            "A cloud file arrived incomplete: {}",
            file.path
        );
        missing.push((file.hash.clone(), source));
    }
    if !missing.is_empty() {
        let mut child = git(dir)
            .args(["hash-object", "-w", "--no-filters", "--stdin-paths"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().context("git hash-object")?;
        let paths = missing
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        let writer = std::thread::spawn(move || -> std::io::Result<()> {
            for path in paths {
                input.write_all(path.as_os_str().as_encoded_bytes())?;
                input.write_all(b"\n")?;
            }
            Ok(())
        });
        let output = child.wait_with_output()?;
        let _ = writer.join();
        ensure!(
            output.status.success(),
            "Couldn't store cloud files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let oids = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        ensure!(oids.len() == missing.len(), "Couldn't store cloud files.");
        for ((hash, _), oid) in missing.into_iter().zip(oids) {
            known.insert(hash, oid);
        }
    }
    let mut entries = Vec::new();
    for file in files {
        if inside_submodule(&file.path) {
            continue;
        }
        let mode = if file.link.is_some() {
            "120000"
        } else if file.mode & 0o111 != 0 {
            "100755"
        } else {
            "100644"
        };
        let oid = known
            .get(&file.hash)
            .context("Couldn't store cloud files.")?;
        entries.extend_from_slice(format!("{mode} {oid}\t{}\0", file.path).as_bytes());
    }
    for (path, oid) in &links {
        entries.extend_from_slice(format!("160000 {oid}\t{path}\0").as_bytes());
    }
    let index = staging.path().join("index");
    let mut child = git(dir)
        .env("GIT_INDEX_FILE", &index)
        .args(["update-index", "--add", "-z", "--index-info"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().context("git update-index")?;
    let writer = std::thread::spawn(move || input.write_all(&entries));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    ensure!(
        output.status.success(),
        "Couldn't record the cloud chat's files: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let mut write_tree = git(dir);
    write_tree.env("GIT_INDEX_FILE", &index).arg("write-tree");
    let tree = run(write_tree)?;
    let mut commit_tree = git(dir);
    commit_tree
        .env("GIT_AUTHOR_NAME", "Zuse")
        .env("GIT_AUTHOR_EMAIL", "cloud-sync@zuse.invalid")
        .env("GIT_COMMITTER_NAME", "Zuse")
        .env("GIT_COMMITTER_EMAIL", "cloud-sync@zuse.invalid")
        .args(["commit-tree", &tree, "-p", parent, "-m", message]);
    run(commit_tree)
}

/// The tree a commit records.
pub(crate) fn tree(dir: &Path, rev: &str) -> Result<String> {
    git_ok(dir, &["rev-parse", &format!("{rev}^{{tree}}")])
}

fn read_tree(dir: &Path, from: &str, to: &str) -> Result<()> {
    git_ok(dir, &["read-tree", "-m", "-u", from, to])
        .map(|_| ())
        .map_err(friendly)
}

/// Explain read-tree's refusals in the user's terms.
fn friendly(error: anyhow::Error) -> anyhow::Error {
    let text = error.to_string();
    let paths = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.contains(':')
                && !line.starts_with("Please")
                && *line != "Aborting"
        })
        .take(3)
        .collect::<Vec<_>>();
    if text.contains("untracked working tree files would be overwritten")
        || text.contains("would be removed by")
        || text.contains("would lose untracked files")
    {
        return anyhow::anyhow!(
            "Files in your folder that git doesn't track are in the way{}. Move them, then try again.",
            if paths.is_empty() {
                String::new()
            } else {
                format!(": {}", paths.join(", "))
            }
        );
    }
    error
}

/// Tracked paths a hard reset would write where something git doesn't
/// track is in the way: a directory where a tracked file belongs, or a file
/// where a tracked directory belongs.
fn obstructions(dir: &Path) -> Result<Vec<String>> {
    let changed = git_ok(dir, &["diff", "--name-only", "-z", "--no-renames", "HEAD"])?;
    let changed = changed
        .split('\0')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return Ok(vec![]);
    }
    let mut blocked = Vec::new();
    for chunk in changed.chunks(500) {
        let mut args = vec!["ls-tree", "-r", "-z", "--name-only", "HEAD", "--"];
        args.extend(chunk.iter().copied());
        let tracked = git_ok(dir, &args)?;
        for path in tracked.split('\0').filter(|p| !p.is_empty()) {
            let target = dir.join(path);
            if std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir()) {
                blocked.push(path.to_owned());
                continue;
            }
            let mut parent = Path::new(path).parent();
            while let Some(part) = parent.filter(|p| !p.as_os_str().is_empty()) {
                if std::fs::symlink_metadata(dir.join(part)).is_ok_and(|m| !m.is_dir()) {
                    blocked.push(path.to_owned());
                    break;
                }
                parent = part.parent();
            }
        }
    }
    Ok(blocked)
}

/// Reset tracked files to HEAD, refusing when that would replace anything
/// git doesn't track (which `git reset --hard` would delete).
fn reset_tracked(dir: &Path) -> Result<()> {
    let blocked = obstructions(dir)?;
    ensure!(
        blocked.is_empty(),
        "Files in your folder that git doesn't track are in the way: {}. Move them, then try again.",
        blocked
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    git_ok(dir, &["reset", "-q", "--hard", "HEAD"]).map(|_| ())
}

/// Whether HEAD is on a branch rather than detached.
fn attached(dir: &Path) -> bool {
    git(dir)
        .args(["symbolic-ref", "-q", "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether the checkout is on `checkpoint`, detached, as sync left it.
pub(crate) fn holds(dir: &Path, checkpoint: &str) -> bool {
    head(dir).is_ok_and(|head| head == checkpoint) && !attached(dir)
}

/// Move the checkout from its saved state to `checkpoint`: the saved changes
/// are already in `saved`, so tracked files are reset, then the checkpoint is
/// checked out detached. On failure the caller puts the checkout back with
/// [`restore`] (without keeping edits), wherever this stopped.
pub(crate) fn enter(dir: &Path, saved: &Saved, checkpoint: &str) -> Result<()> {
    ensure!(
        head(dir)? == saved.head,
        "This folder changed while sync was starting. Try again."
    );
    pin(dir, REF_CHECKPOINT, checkpoint)?;
    reset_tracked(dir)?;
    read_tree(dir, "HEAD", checkpoint)?;
    git_ok(
        dir,
        &[
            "update-ref",
            "--no-deref",
            "-m",
            "Zuse: sync a cloud chat",
            "HEAD",
            checkpoint,
        ],
    )?;
    Ok(())
}

/// Why the checkout can't take the next checkpoint now, if it can't: it
/// left `at` (another branch or commit), or tracked files were edited here.
pub(crate) fn local_edits(dir: &Path, at: &str) -> Result<Option<&'static str>> {
    if head(dir)? != at || attached(dir) {
        return Ok(Some(
            "This folder moved to another branch or commit, so sync paused.",
        ));
    }
    if has_tracked_changes(dir)? {
        return Ok(Some(
            "Files in this folder were edited here, so sync paused to keep them.",
        ));
    }
    Ok(None)
}

/// Move a clean, synced checkout from checkpoint `from` to `to`.
pub(crate) fn advance(dir: &Path, from: &str, to: &str) -> Result<()> {
    if let Some(reason) = local_edits(dir, from)? {
        bail!("{reason}");
    }
    read_tree(dir, from, to)?;
    git_ok(
        dir,
        &[
            "update-ref",
            "--no-deref",
            "-m",
            "Zuse: sync a cloud chat",
            "HEAD",
            to,
            from,
        ],
    )?;
    pin(dir, REF_CHECKPOINT, to)
}

/// Bring a paused checkout back to `checkpoint` after the user chose to
/// continue. Their edits are already in the stash list; tracked files are
/// reset, untracked and ignored ones stay.
pub(crate) fn return_to(dir: &Path, checkpoint: &str) -> Result<()> {
    reset_tracked(dir)?;
    if head(dir)? != checkpoint {
        read_tree(dir, "HEAD", checkpoint)?;
    }
    // Detached even when a branch was made at the checkpoint itself.
    git_ok(
        dir,
        &[
            "update-ref",
            "--no-deref",
            "-m",
            "Zuse: sync a cloud chat",
            "HEAD",
            checkpoint,
        ],
    )?;
    Ok(())
}

/// Put saved changes back on the checked-out commit. Returns false when
/// they no longer apply cleanly; then they are kept in the stash list.
fn apply_changes(dir: &Path, changes: &str) -> Result<bool> {
    if git_ok(dir, &["stash", "apply", "--index", "-q", changes]).is_ok() {
        return Ok(true);
    }
    if git_ok(dir, &["stash", "apply", "-q", changes]).is_ok() {
        return Ok(true);
    }
    // Never lost: the stash list keeps them.
    reset_tracked(dir)?;
    git_ok(
        dir,
        &[
            "stash",
            "store",
            "-m",
            "Zuse: your changes before syncing a cloud chat",
            changes,
        ],
    )?;
    Ok(false)
}

/// What restoring did besides putting the checkout back.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Restored {
    /// Edits made here while synced were kept in the stash list.
    pub kept_edits: bool,
    /// The branch moved meanwhile and the earlier changes no longer applied
    /// on it, so they are in the stash list instead.
    pub changes_in_stash_list: bool,
}

/// Put the checkout back exactly as saved: the branch (or detached HEAD),
/// its commit and the tracked changes it had. With `keep_edits`, edits made
/// while synced go to the stash list first (without it, tracked differences
/// are sync's own, from an interrupted step). Safe to repeat after an
/// interruption.
pub(crate) fn restore(dir: &Path, saved: &Saved, keep_edits: bool) -> Result<Restored> {
    let mut restored = Restored::default();
    if keep_edits && has_tracked_changes(dir)? {
        restored.kept_edits =
            keep_in_stash_list(dir, "Zuse: edits made while synced to a cloud chat")?;
    }
    reset_tracked(dir)?;
    // The branch's own tip, unless it was deleted meanwhile (then it comes
    // back at the saved commit).
    let target = match &saved.branch {
        Some(branch) => commit(dir, branch)?.unwrap_or_else(|| saved.head.clone()),
        None => saved.head.clone(),
    };
    if head(dir)? != target {
        read_tree(dir, "HEAD", &target)?;
    }
    match &saved.branch {
        Some(branch) => {
            if commit(dir, branch)?.is_none() {
                git_ok(dir, &["update-ref", branch, &target])?;
            }
            git_ok(dir, &["symbolic-ref", "HEAD", branch])?;
        }
        None => {
            git_ok(
                dir,
                &[
                    "update-ref",
                    "--no-deref",
                    "-m",
                    "Zuse: restore after cloud chat sync",
                    "HEAD",
                    &target,
                ],
            )?;
        }
    }
    if let Some(changes) = &saved.changes {
        restored.changes_in_stash_list = !apply_changes(dir, changes)?;
    }
    unpin(dir);
    Ok(restored)
}

/// How a chat's folder was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Made {
    /// A worktree of the user's clone (shares its objects).
    Worktree,
    /// A shallow clone of the repository.
    Clone,
    /// An empty repository (the clone failed): every file arrives with the
    /// first snapshot.
    Empty,
}

/// Whether `path` is a chat folder that has been made: a git folder with a
/// commit checked out.
pub(crate) fn is_folder(path: &Path) -> bool {
    path.join(".git").exists() && head(path).is_ok()
}

/// The top folder of a working clone `path` is in, if it is in one.
pub(crate) fn clone_root(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let top = PathBuf::from(git_ok(&canonical, &["rev-parse", "--show-toplevel"]).ok()?);
    let top = top.canonicalize().ok()?;
    (git_ok(&top, &["rev-parse", "--is-bare-repository"]).ok()? == "false" && head(&top).is_ok())
        .then_some(top)
}

/// Options that keep a folder being made from running the repository's
/// hooks or asking for credentials.
const QUIET: [&str; 2] = ["-c", "core.hooksPath=/dev/null"];

/// Make the chat's folder at `path` (absent, or an empty folder): a
/// worktree of `source` at its current commit, detached, else a shallow
/// clone of `url`, else an empty repository with `url` as its origin.
pub(crate) fn create_folder(path: &Path, source: Option<&Path>, url: &str) -> Result<Made> {
    ensure!(path.is_absolute(), "Choose an absolute folder.");
    if path.exists() {
        ensure!(
            std::fs::read_dir(path)?.next().is_none(),
            "{} already exists and isn't a folder Zuse made for this chat.",
            path.display()
        );
        std::fs::remove_dir(path)?;
    }
    let parent = path.parent().context("Choose an absolute folder.")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Couldn't create {}.", parent.display()))?;
    let target = path
        .to_str()
        .context("The folder's path isn't valid text.")?;
    if let Some(source) = source {
        let made = head(source).and_then(|base| {
            let mut args = QUIET.to_vec();
            args.extend(["worktree", "add", "--quiet", "--detach", target, &base]);
            git_ok(source, &args)
        });
        match made {
            Ok(_) if is_folder(path) => return Ok(Made::Worktree),
            Ok(_) => tracing::info!("checkout sync: the worktree came out incomplete"),
            Err(error) => tracing::info!(%error, "checkout sync: no worktree, cloning"),
        }
        let _ = remove_folder(path);
    }
    let mut args = QUIET.to_vec();
    args.extend([
        "clone",
        "--quiet",
        "--depth",
        "1",
        "--no-tags",
        "--single-branch",
        url,
        target,
    ]);
    let mut clone = git(parent);
    clone
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .args(&args);
    match run(clone) {
        Ok(_) if is_folder(path) => return Ok(Made::Clone),
        Ok(_) => tracing::info!("checkout sync: the clone came out empty"),
        Err(error) => tracing::info!(%error, "checkout sync: clone failed, starting empty"),
    }
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    git_ok(parent, &["init", "--quiet", target])?;
    git_ok(path, &["remote", "add", "origin", url])?;
    let tree = git_ok(path, &["write-tree"])?;
    let mut commit_tree = git(path);
    commit_tree
        .env("GIT_AUTHOR_NAME", "Zuse")
        .env("GIT_AUTHOR_EMAIL", "cloud-sync@zuse.invalid")
        .env("GIT_COMMITTER_NAME", "Zuse")
        .env("GIT_COMMITTER_EMAIL", "cloud-sync@zuse.invalid")
        .args(["commit-tree", &tree, "-m", "Start of a cloud chat's folder"]);
    let commit = run(commit_tree)?;
    git_ok(
        path,
        &[
            "update-ref",
            "--no-deref",
            "-m",
            "Zuse: start a cloud chat's folder",
            "HEAD",
            &commit,
        ],
    )?;
    Ok(Made::Empty)
}

/// Whether the folder holds nothing but the chat's files: on `checkpoint`,
/// detached, with no tracked change and no file git would track. (Ignored
/// files, such as build output, don't count.)
pub(crate) fn untouched(dir: &Path, checkpoint: &str) -> bool {
    holds(dir, checkpoint)
        && has_tracked_changes(dir).is_ok_and(|changed| !changed)
        && git_ok(
            dir,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=all",
            ],
        )
        .is_ok_and(|status| status.is_empty())
}

/// Remove a chat's folder: a worktree is removed from its clone too (the
/// clone itself, its branches and its other worktrees stay). Refuses a
/// folder that isn't a git folder (or empty).
pub(crate) fn remove_folder(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let dot_git = path.join(".git");
    ensure!(
        std::fs::symlink_metadata(path)?.is_dir()
            && (dot_git.exists() || std::fs::read_dir(path)?.next().is_none()),
        "{} isn't a folder Zuse made for a chat.",
        path.display()
    );
    let common = dot_git
        .is_file()
        .then(|| {
            git_ok(
                path,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok()
        })
        .flatten()
        .map(PathBuf::from);
    if let (Some(common), Some(target)) = (&common, path.to_str()) {
        let _ = git_ok(
            common,
            &["worktree", "remove", "--force", "--force", target],
        );
    }
    if path.exists() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Couldn't delete {}.", path.display()))?;
    }
    if let Some(common) = &common {
        let _ = git_ok(common, &["worktree", "prune"]);
    }
    // The repository's group folder goes with its last chat.
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}
