//! Git operations on the user's own checkout: save how it is, move it to a
//! cloud chat's checkpoint, keep it there, and put it back exactly. Untracked
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
/// reachable, so `git gc` can never collect them while synced.
pub(crate) const REF_HEAD: &str = "refs/zuse/checkout-sync/head";
pub(crate) const REF_CHANGES: &str = "refs/zuse/checkout-sync/changes";
pub(crate) const REF_CHECKPOINT: &str = "refs/zuse/checkout-sync/checkpoint";
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
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .stdin(Stdio::null());
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

/// The checkout's root, which `path` must be, with no merge, rebase or
/// similar operation in progress.
pub(crate) fn checkout_root(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "Choose an absolute folder.");
    let canonical = path
        .canonicalize()
        .context("This folder doesn't exist on this computer.")?;
    let top = git_ok(&canonical, &["rev-parse", "--show-toplevel"])
        .map_err(|_| anyhow::anyhow!("This folder isn't a git checkout."))?;
    let top = PathBuf::from(top)
        .canonicalize()
        .context("This folder isn't a git checkout.")?;
    ensure!(
        top == canonical,
        "Choose the repository's top folder ({}).",
        top.display()
    );
    ensure!(
        git_ok(&top, &["rev-parse", "--is-bare-repository"])? == "false",
        "This folder isn't a working checkout."
    );
    let git_dir = PathBuf::from(git_ok(
        &top,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    )?);
    for (marker, what) in [
        ("MERGE_HEAD", "a merge"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase"),
        ("BISECT_LOG", "a bisect"),
    ] {
        ensure!(
            !git_dir.join(marker).exists(),
            "Finish or abort {what} in {} first.",
            top.display()
        );
    }
    head(&top)?;
    Ok(top)
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

/// Move the checkout from its saved state to `checkpoint`: the saved changes
/// are already in `saved`, so tracked files are reset, then the checkpoint is
/// checked out detached. Fails without changing anything left unsaved; on
/// failure the saved changes are put back.
pub(crate) fn enter(dir: &Path, saved: &Saved, checkpoint: &str) -> Result<()> {
    ensure!(
        head(dir)? == saved.head,
        "This folder changed while sync was starting. Try again."
    );
    pin(dir, REF_CHECKPOINT, checkpoint)?;
    git_ok(dir, &["reset", "-q", "--hard", "HEAD"])?;
    if let Err(error) = read_tree(dir, "HEAD", checkpoint) {
        if let Some(changes) = &saved.changes {
            apply_changes(dir, changes)?;
        }
        unpin(dir);
        return Err(error);
    }
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
    if head(dir)? != at {
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
    git_ok(dir, &["reset", "-q", "--hard", "HEAD"])?;
    if head(dir)? != checkpoint {
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
    }
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
    git_ok(dir, &["reset", "-q", "--hard", "HEAD"])?;
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
/// its commit and the tracked changes it had. Edits made while synced go to
/// the stash list first. Safe to repeat after an interruption.
pub(crate) fn restore(dir: &Path, saved: &Saved) -> Result<Restored> {
    let mut restored = Restored::default();
    if has_tracked_changes(dir)? {
        restored.kept_edits =
            keep_in_stash_list(dir, "Zuse: edits made while synced to a cloud chat")?;
    }
    git_ok(dir, &["reset", "-q", "--hard", "HEAD"])?;
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
