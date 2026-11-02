//! A branch beside the one it merges into: the commits each side made since
//! they forked, and whether merging them would conflict. Read-only; computed
//! from local refs (optionally after fetching the base) without touching the
//! working tree, the index or any branch.
use crate::GitHistoryCommit;
use serde::{Deserialize, Serialize};

/// `GetBranchGraph` params.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchGraphRequest {
    pub cwd: String,
    /// The branch the checkout merges into, e.g. `main`. Resolved against the
    /// remote-tracking ref first (`origin/main`), then a local branch.
    pub base: String,
    /// Fetch the base from its remote first, so conflicts reflect where it
    /// is now. Best effort: a failed fetch falls back to the local ref.
    #[serde(default)]
    pub fetch: bool,
    /// Most commits to return across both sides.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BranchGraph {
    /// The ref the head was compared with, e.g. `origin/main`.
    pub base_ref: String,
    pub head_branch: String,
    pub head_sha: Option<String>,
    /// Where the branch left the base.
    pub fork_sha: Option<String>,
    /// Commits on the head but not the base.
    pub ahead: usize,
    /// Commits on the base but not the head.
    pub behind: usize,
    /// Both sides since the fork, newest first in topological order, ending
    /// with the fork commit itself. Parents outside the list are dropped.
    pub commits: Vec<GitHistoryCommit>,
    /// SHAs in [`Self::commits`] that only the head has.
    pub head_shas: Vec<String>,
    /// `user.email` in this checkout, to tell the user's commits apart.
    pub self_email: Option<String>,
    /// Files changed in the working tree but not committed.
    pub uncommitted: usize,
    /// `None` when Git can't preview merges (older than 2.38) or the head
    /// has no commits.
    pub merge: Option<MergePreview>,
}

/// What merging the base into the head would do, from `git merge-tree`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MergePreview {
    pub clean: bool,
    /// Conflicting files, each with up to a few of its conflicting regions.
    pub conflicts: Vec<MergeConflict>,
    /// Conflicting files beyond those listed.
    pub more_files: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MergeConflict {
    pub path: String,
    /// Who last changed the file on the base side.
    pub base_author: Option<String>,
    pub hunks: Vec<ConflictHunk>,
    /// A conflict with no text regions: binary, deleted on one side, or
    /// renamed. Its description from Git.
    pub note: Option<String>,
}

/// One conflicting region: where it starts in the head's file, and the first
/// line each side has there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConflictHunk {
    pub line: u32,
    pub head: String,
    pub base: String,
}
