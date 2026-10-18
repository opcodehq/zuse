//! Bounded Git operations serialized per checkout. Git owns index/ref locking.
use crate::{EngineError, Repos};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use zuse_protocol::{
    GitFileState,
    git_workspace::{GitOperation, GitWorkspace},
};
const LIMIT: usize = 2 * 1024 * 1024;
fn error(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
async fn drain(
    mut stream: impl AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = vec![];
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < n;
    }
    Ok((bytes, truncated))
}
async fn git(root: &Path, args: &[&str], input: &[u8]) -> Result<(Vec<u8>, bool), EngineError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let done = async {
        let write = async {
            stdin.write_all(input).await?;
            drop(stdin);
            Ok::<_, std::io::Error>(())
        };
        let (out, err, status, _) = tokio::try_join!(
            drain(stdout, LIMIT),
            drain(stderr, 16 * 1024),
            child.wait(),
            write
        )?;
        if !status.success() {
            let message = zuse_providers::redact::redact_output(&String::from_utf8_lossy(&err.0));
            return Err(error(if message.trim().is_empty() {
                "Git could not complete the action."
            } else {
                message.trim()
            }));
        }
        Ok(out)
    };
    tokio::time::timeout(Duration::from_secs(120), done)
        .await
        .map_err(|_| error("Git timed out. Refresh its state before trying again."))?
}
async fn text(root: &Path, args: &[&str]) -> Result<String, EngineError> {
    let (bytes, partial) = git(root, args, &[]).await?;
    if partial {
        return Err(error("Git metadata exceeds the display limit."));
    }
    String::from_utf8(bytes)
        .map(|s| s.trim().into())
        .map_err(|_| error("Git metadata is not UTF-8."))
}
pub(crate) async fn snapshot(
    repos: &Repos,
    root: &Path,
    chat: &str,
) -> Result<GitWorkspace, EngineError> {
    let identity = repos.checkout_identity(root).await?;
    let (bytes, partial) = git(
        &identity.root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        &[],
    )
    .await?;
    let (files, complete) = crate::diff_sync::git_status::parse(&bytes, partial);
    let branch = repos.current_branch(&identity.root).await?;
    let head_sha = repos.head_sha(&identity.root).await?;
    let upstream = text(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .await
    .ok();
    let (ahead, behind) = if upstream.is_some() {
        let counts = text(
            root,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )
        .await?;
        let mut parts = counts.split_whitespace();
        (
            parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
        )
    } else {
        (0, 0)
    };
    let index = text(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )
    .await?;
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(&(
            &identity.id,
            &branch,
            &head_sha,
            &upstream,
            ahead,
            behind,
            &files,
            complete,
        ))
        .unwrap(),
    );
    match tokio::fs::File::open(&index).await {
        Ok(mut file) => {
            let mut buffer = [0u8; 8192];
            let mut total = 0usize;
            loop {
                let n = file.read(&mut buffer).await?;
                if n == 0 {
                    break;
                }
                total = total.saturating_add(n);
                if total > 128 * 1024 * 1024 {
                    return Err(error("The Git index is too large for this control."));
                }
                hash.update(&buffer[..n]);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(GitWorkspace {
        chat_id: chat.into(),
        checkout_id: identity.id,
        cwd: identity.root.to_string_lossy().into(),
        branch,
        head_sha,
        upstream,
        ahead,
        behind,
        files,
        complete,
        revision: crate::repos::hex(&hash.finalize()),
        created_pull_request_url: None,
    })
}
pub(crate) async fn apply(
    root: &Path,
    current: &GitWorkspace,
    operation: &GitOperation,
) -> Result<(), EngineError> {
    if !current.complete {
        return Err(error(
            "Git status is incomplete. Use your Git client to review this checkout.",
        ));
    }
    match operation {
        GitOperation::CreatePullRequest { .. } => {
            return Err(error(
                "PR creation must use the host source-control workflow.",
            ));
        }
        GitOperation::Stage { paths } | GitOperation::Unstage { paths } => {
            if paths.is_empty() || paths.len() > 10000 {
                return Err(error("Choose between one and 10,000 changed files."));
            }
            let mut selected = BTreeSet::new();
            for path in paths {
                let file = current
                    .files
                    .iter()
                    .find(|file| &file.path == path)
                    .ok_or_else(|| error("Changed files moved. Refresh before staging."))?;
                for path in std::iter::once(&file.path).chain(file.old_path.iter()) {
                    if path.is_empty()
                        || path.as_bytes().contains(&0)
                        || Path::new(path).is_absolute()
                        || Path::new(path)
                            .components()
                            .any(|p| !matches!(p, std::path::Component::Normal(_)))
                    {
                        return Err(error("Invalid changed-file path."));
                    }
                    selected.insert(path.as_str());
                }
            }
            let input: Vec<u8> = selected
                .into_iter()
                .flat_map(|path| path.as_bytes().iter().copied().chain([0]))
                .collect();
            let args = if matches!(operation, GitOperation::Stage { .. }) {
                vec![
                    "add",
                    "--all",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]
            } else if current.head_sha.is_some() {
                vec![
                    "restore",
                    "--staged",
                    "--source=HEAD",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]
            } else {
                vec![
                    "rm",
                    "--cached",
                    "--force",
                    "--ignore-unmatch",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ]
            };
            git(root, &args, &input).await?;
        }
        GitOperation::Commit { message } => {
            if message.trim().is_empty()
                || message.len() > 64 * 1024
                || message.as_bytes().contains(&0)
            {
                return Err(error("Enter a commit message of at most 64 KiB."));
            }
            if !current
                .files
                .iter()
                .any(|f| !matches!(f.index, GitFileState::Unchanged | GitFileState::Untracked))
            {
                return Err(error("Stage changes before committing."));
            }
            git(root, &["commit", "--file=-"], message.as_bytes()).await?;
        }
        GitOperation::Push => {
            if current.branch == "HEAD" {
                return Err(error("Create or switch to a branch before pushing."));
            }
            if current.upstream.is_some() {
                git(root, &["push"], &[]).await?;
            } else {
                let target = format!("HEAD:refs/heads/{}", current.branch);
                git(root, &["push", "--set-upstream", "origin", &target], &[]).await?;
            }
        }
        GitOperation::Pull => {
            git(root, &["pull", "--no-rebase", "--ff-only"], &[]).await?;
        }
        GitOperation::CreateBranch { name } | GitOperation::SwitchBranch { name } => {
            if name.is_empty() || name.len() > 255 || name.starts_with('-') {
                return Err(error("Enter a valid branch name."));
            }
            git(root, &["check-ref-format", "--branch", name], &[]).await?;
            let args = if matches!(operation, GitOperation::CreateBranch { .. }) {
                vec!["switch", "--create", name]
            } else {
                vec!["switch", "--", name]
            };
            git(root, &args, &[]).await?;
        }
    }
    Ok(())
}
