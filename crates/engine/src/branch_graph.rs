//! A branch beside its base since they forked, plus a `git merge-tree`
//! preview. Read-only: never touches the working tree, the index, HEAD or a
//! local branch (a requested fetch only moves the base's remote-tracking ref).
use crate::{EngineError, git_workspace::drain};
use std::{collections::HashSet, path::Path, process::Stdio, time::Duration};
use zuse_protocol::{
    GitHistoryCommit, GitHistoryRef, GitHistoryRefKind,
    branch_graph::{BranchGraph, BranchGraphRequest, ConflictHunk, MergeConflict, MergePreview},
};
const LIMIT: usize = 2 * 1024 * 1024;
const BLOB_LIMIT: usize = 512 * 1024;
const WAIT: Duration = Duration::from_secs(30);
const FETCH_WAIT: Duration = Duration::from_secs(20);
const DEFAULT_COMMITS: usize = 24;
const MAX_COMMITS: usize = 60;
const MAX_FILES: usize = 6;
const MAX_HUNKS: usize = 3;
const LINE_CHARS: usize = 160;
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%s%x00%an%x00%ae%x00%aI%x00%D%x00";
const NO_TEXT: &str = "Binary file or a delete/rename conflict";
fn error(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
struct Run {
    out: Vec<u8>,
    truncated: bool,
    code: Option<i32>,
    err: String,
}
/// One bounded Git command; unlike `git_workspace::git` a non-zero exit is
/// returned, not raised (`merge-tree` exits 1 on conflicts).
async fn run(root: &Path, args: &[&str], limit: usize, wait: Duration) -> Result<Run, EngineError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let done = async {
        let ((out, truncated), (err, _), status) =
            tokio::try_join!(drain(stdout, limit), drain(stderr, 16 * 1024), child.wait())?;
        Ok::<_, std::io::Error>(Run {
            out,
            truncated,
            code: status.code(),
            err: String::from_utf8_lossy(&err).into_owned(),
        })
    };
    Ok(tokio::time::timeout(wait, done)
        .await
        .map_err(|_| error("Git timed out."))??)
}
/// Trimmed stdout of a command that must succeed.
async fn text(root: &Path, args: &[&str]) -> Result<String, EngineError> {
    let run = run(root, args, LIMIT, WAIT).await?;
    if run.code != Some(0) {
        let message = zuse_providers::redact::redact_output(&run.err);
        return Err(error(if message.trim().is_empty() {
            "Git could not read the branch."
        } else {
            message.trim()
        }));
    }
    Ok(String::from_utf8_lossy(&run.out).trim().into())
}
/// Trimmed stdout when the command succeeds with output.
async fn optional(root: &Path, args: &[&str]) -> Option<String> {
    text(root, args).await.ok().filter(|s| !s.is_empty())
}
async fn commit(root: &Path, spec: &str) -> Option<String> {
    optional(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{spec}^{{commit}}"),
        ],
    )
    .await
}
pub async fn branch_graph(
    root: &Path,
    request: &BranchGraphRequest,
) -> Result<BranchGraph, EngineError> {
    let base = request.base.trim();
    if base.is_empty() || base.starts_with('-') {
        return Err(error("Choose a base branch to compare with."));
    }
    text(root, &["rev-parse", "--is-inside-work-tree"]).await?;
    let head_branch = match optional(root, &["rev-parse", "--abbrev-ref", "HEAD"]).await {
        Some(name) => name,
        // Unborn branch: no commit to abbreviate yet.
        None => optional(root, &["symbolic-ref", "--short", "-q", "HEAD"])
            .await
            .unwrap_or_else(|| "HEAD".into()),
    };
    let remotes = text(root, &["remote"]).await.unwrap_or_default();
    let remotes: Vec<&str> = remotes.lines().map(str::trim).collect();
    let mut remote = None;
    for name in [
        optional(root, &["config", "--get", &format!("branch.{base}.remote")]).await,
        optional(
            root,
            &["config", "--get", &format!("branch.{head_branch}.remote")],
        )
        .await,
        Some("origin".into()),
    ]
    .into_iter()
    .flatten()
    {
        if remotes.contains(&name.as_str()) {
            remote = Some(name);
            break;
        }
    }
    if let (true, Some(remote)) = (request.fetch, &remote) {
        // Best effort; the local remote-tracking ref stands in on failure.
        let _ = run(
            root,
            &["fetch", "--quiet", "--no-tags", remote, base],
            64 * 1024,
            FETCH_WAIT,
        )
        .await;
    }
    let mut candidates = vec![];
    if let Some(remote) = &remote {
        candidates.push(format!("refs/remotes/{remote}/{base}"));
    }
    candidates.push(format!("refs/heads/{base}"));
    candidates.push(format!("refs/remotes/{base}"));
    let mut resolved = None;
    for full in candidates {
        if let Some(sha) = commit(root, &full).await {
            resolved = Some((full, sha));
            break;
        }
    }
    let Some((full, base_sha)) = resolved else {
        return Err(error(&format!("Base branch {base} was not found.")));
    };
    let base_ref = full
        .strip_prefix("refs/remotes/")
        .or_else(|| full.strip_prefix("refs/heads/"))
        .unwrap_or(&full)
        .to_string();
    let (head_sha, self_email, status) = tokio::join!(
        commit(root, "HEAD"),
        optional(root, &["config", "--get", "user.email"]),
        run(root, &["status", "--porcelain"], LIMIT, WAIT),
    );
    let uncommitted = status
        .ok()
        .filter(|run| run.code == Some(0))
        .map_or(0, |run| {
            run.out
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .count()
        });
    let mut graph = BranchGraph {
        base_ref,
        head_branch,
        head_sha: head_sha.clone(),
        self_email,
        uncommitted,
        ..Default::default()
    };
    let Some(head_sha) = head_sha else {
        graph.behind = text(root, &["rev-list", "--count", &base_sha])
            .await?
            .parse()
            .unwrap_or(0);
        return Ok(graph);
    };
    let counts = text(
        root,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{head_sha}...{base_sha}"),
        ],
    )
    .await?;
    let mut counts = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
    graph.ahead = counts.next().unwrap_or(0);
    graph.behind = counts.next().unwrap_or(0);
    let only_head = format!("{base_sha}..{head_sha}");
    let all_args = ["rev-list", &only_head];
    let first_args = ["rev-list", "--first-parent", &only_head];
    let (all, first_parent) = tokio::join!(text(root, &all_args), text(root, &first_args));
    let all = all?;
    // The first-parent chain's oldest commit left the base at its first
    // parent; that stays the fork after the base is merged into the branch.
    graph.fork_sha = match first_parent?.lines().last() {
        Some(oldest) => {
            optional(
                root,
                &["rev-parse", "--verify", "--quiet", &format!("{oldest}^1")],
            )
            .await
        }
        None => optional(root, &["merge-base", &head_sha, &base_sha]).await,
    };
    let limit = request
        .limit
        .unwrap_or(DEFAULT_COMMITS)
        .clamp(1, MAX_COMMITS);
    let count = format!("--max-count={limit}");
    let mut commits = match &graph.fork_sha {
        Some(fork) => {
            let mut commits =
                log(root, &[&count, &head_sha, &base_sha, &format!("^{fork}")]).await?;
            commits.extend(log(root, &["--max-count=1", fork]).await?);
            commits
        }
        None => log(root, &[&count, &head_sha, &base_sha]).await?,
    };
    let shas: HashSet<String> = commits.iter().map(|c| c.sha.clone()).collect();
    for commit in &mut commits {
        commit.parent_shas.retain(|sha| shas.contains(sha));
    }
    let head_only: HashSet<&str> = all.lines().collect();
    graph.head_shas = commits
        .iter()
        .filter(|c| head_only.contains(c.sha.as_str()))
        .map(|c| c.sha.clone())
        .collect();
    graph.commits = commits;
    graph.merge = merge_preview(root, &head_sha, &base_sha, graph.fork_sha.as_deref()).await;
    Ok(graph)
}
/// Commits for `revs`, newest first in topological order.
async fn log(root: &Path, revs: &[&str]) -> Result<Vec<GitHistoryCommit>, EngineError> {
    let mut args = vec![
        "log",
        "--topo-order",
        "--no-color",
        "--no-show-signature",
        "--no-patch",
        "--decorate=full",
        "--decorate-refs=refs/heads",
        "--decorate-refs=refs/remotes",
        "--decorate-refs=refs/tags",
        "--decorate-refs-exclude=refs/remotes/*/HEAD",
        LOG_FORMAT,
    ];
    args.extend(revs);
    args.push("--");
    Ok(parse_log(&text(root, &args).await?))
}
/// `None` when this Git has no `merge-tree --write-tree` (before 2.38).
async fn merge_preview(
    root: &Path,
    head: &str,
    base: &str,
    fork: Option<&str>,
) -> Option<MergePreview> {
    let args = [
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        "-z",
        head,
        base,
    ];
    let run = run(root, &args, LIMIT, WAIT).await.ok()?;
    match run.code {
        Some(0) => {
            return Some(MergePreview {
                clean: true,
                ..Default::default()
            });
        }
        Some(1) => {}
        _ => return None,
    }
    let out = String::from_utf8_lossy(&run.out);
    let mut fields = out.split('\0');
    let tree = fields.next()?.trim().to_string();
    let mut paths: Vec<&str> = vec![];
    for path in fields.take_while(|path| !path.is_empty()) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if run.truncated && paths.is_empty() {
        return None;
    }
    let range = fork.map_or_else(|| base.to_string(), |fork| format!("{fork}..{base}"));
    let conflicts = futures::future::join_all(
        paths
            .iter()
            .take(MAX_FILES)
            .map(|path| conflict(root, &tree, &range, path)),
    )
    .await;
    Some(MergePreview {
        clean: false,
        more_files: paths.len().saturating_sub(MAX_FILES),
        conflicts,
    })
}
async fn conflict(root: &Path, tree: &str, range: &str, path: &str) -> MergeConflict {
    let blob = format!("{tree}:{path}");
    let blob_args = ["cat-file", "blob", &blob];
    let author_args = ["log", "-1", "--format=%an", range, "--", path];
    let (content, base_author) = tokio::join!(
        run(root, &blob_args, BLOB_LIMIT, WAIT),
        optional(root, &author_args),
    );
    let hunks = content
        .ok()
        .filter(|run| run.code == Some(0) && !run.out.contains(&0))
        .map(|run| conflict_hunks(&String::from_utf8_lossy(&run.out), MAX_HUNKS))
        .unwrap_or_default();
    MergeConflict {
        path: path.into(),
        base_author,
        note: hunks.is_empty().then(|| NO_TEXT.into()),
        hunks,
    }
}
fn is_marker(line: &str, marker: &str) -> bool {
    line.strip_prefix(marker)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}
fn first_line(slot: &mut Option<String>, line: &str) {
    let line = line.trim();
    if slot.is_none() && !line.is_empty() {
        *slot = Some(line.chars().take(LINE_CHARS).collect());
    }
}
/// Conflict regions in a file holding merge markers. `line` counts only the
/// lines the head's version has; diff3 ancestor sections are skipped.
pub(crate) fn conflict_hunks(text: &str, max: usize) -> Vec<ConflictHunk> {
    enum Side {
        Outside,
        Head,
        Ancestor,
        Base,
    }
    let mut hunks = vec![];
    let mut side = Side::Outside;
    let mut line_no = 0u32;
    let (mut start, mut head, mut base) = (0, None, None);
    for line in text.lines() {
        side = match side {
            Side::Outside if is_marker(line, "<<<<<<<") => {
                (start, head, base) = (line_no + 1, None, None);
                Side::Head
            }
            Side::Outside => {
                line_no += 1;
                Side::Outside
            }
            Side::Head if is_marker(line, "|||||||") => Side::Ancestor,
            Side::Head | Side::Ancestor if is_marker(line, "=======") => Side::Base,
            Side::Head => {
                line_no += 1;
                first_line(&mut head, line);
                Side::Head
            }
            Side::Ancestor => Side::Ancestor,
            Side::Base if is_marker(line, ">>>>>>>") => {
                hunks.push(ConflictHunk {
                    line: start,
                    head: head.take().unwrap_or_default(),
                    base: base.take().unwrap_or_default(),
                });
                if hunks.len() >= max {
                    break;
                }
                Side::Outside
            }
            Side::Base => {
                first_line(&mut base, line);
                Side::Base
            }
        };
    }
    hunks
}
/// Records from [`LOG_FORMAT`]; `%D` decorations become refs.
fn parse_log(output: &str) -> Vec<GitHistoryCommit> {
    let fields: Vec<&str> = output.split('\0').collect();
    fields
        .chunks(7)
        .filter(|record| record.len() == 7)
        .filter_map(|record| {
            let sha = record[0].trim_start_matches(['\r', '\n']);
            if sha.is_empty() {
                return None;
            }
            let bounded = |value: &str, max: usize| value.chars().take(max).collect::<String>();
            Some(GitHistoryCommit {
                sha: sha.into(),
                parent_shas: record[1]
                    .split_ascii_whitespace()
                    .map(str::to_string)
                    .collect(),
                subject: bounded(record[2], 4_096),
                author_name: bounded(record[3], 512),
                author_email: bounded(record[4], 512),
                authored_at: bounded(record[5], 128),
                refs: parse_refs(record[6]),
            })
        })
        .collect()
}
fn parse_refs(decorations: &str) -> Vec<GitHistoryRef> {
    let mut refs: Vec<GitHistoryRef> = decorations
        .split(", ")
        .filter_map(|name| {
            let name = name.trim();
            let name = name.strip_prefix("HEAD -> ").unwrap_or(name);
            let name = name.strip_prefix("tag: ").unwrap_or(name);
            let (kind, label) = if let Some(label) = name.strip_prefix("refs/heads/") {
                (GitHistoryRefKind::Branch, label)
            } else if let Some(label) = name.strip_prefix("refs/remotes/") {
                (GitHistoryRefKind::Remote, label)
            } else {
                (GitHistoryRefKind::Tag, name.strip_prefix("refs/tags/")?)
            };
            Some(GitHistoryRef {
                kind,
                label: label.chars().take(1_024).collect(),
            })
        })
        .collect();
    let order = |kind| match kind {
        GitHistoryRefKind::Branch => 0,
        GitHistoryRefKind::Tag => 1,
        GitHistoryRefKind::Remote => 2,
    };
    refs.sort_by(|a, b| {
        order(a.kind)
            .cmp(&order(b.kind))
            .then_with(|| a.label.cmp(&b.label))
    });
    refs
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hunks_count_head_lines_and_skip_ancestors() {
        let text = "one\n<<<<<<< HEAD\n\n  mine  \n=======\ntheirs\n>>>>>>> abc\ntwo\n\
                    <<<<<<< HEAD\nx\ny\n||||||| base\nancestor\n=======\n\nz\n>>>>>>> abc\n\
                    <<<<<<< HEAD\n=======\nadded\n>>>>>>> abc\n<<<<<<< HEAD\nlast\n=======\n>>>>>>> abc\n";
        let hunks = conflict_hunks(text, 3);
        let hunk = |line, head: &str, base: &str| ConflictHunk {
            line,
            head: head.into(),
            base: base.into(),
        };
        assert_eq!(
            hunks,
            vec![
                hunk(2, "mine", "theirs"),
                hunk(5, "x", "z"),
                hunk(7, "", "added")
            ]
        );
        assert!(conflict_hunks("======= not a marker\n<<<<<<<< nor this\n", 3).is_empty());
        let long = format!("<<<<<<< HEAD\n{}\n=======\nb\n>>>>>>> x\n", "a".repeat(400));
        assert_eq!(conflict_hunks(&long, 3)[0].head.len(), LINE_CHARS);
    }
    #[test]
    fn decorations_become_refs() {
        let refs =
            parse_refs("HEAD -> refs/heads/feat, refs/remotes/origin/feat, tag: refs/tags/v1");
        let labels: Vec<_> = refs.iter().map(|r| (r.kind, r.label.as_str())).collect();
        assert_eq!(
            labels,
            [
                (GitHistoryRefKind::Branch, "feat"),
                (GitHistoryRefKind::Tag, "v1"),
                (GitHistoryRefKind::Remote, "origin/feat"),
            ]
        );
        assert!(parse_refs("HEAD").is_empty() && parse_refs("").is_empty());
    }
}
