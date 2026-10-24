//! Source choices reuse checked-out branches and never switch the project folder.
use super::*;
use zuse_protocol::creation_source::{CheckoutSource, Row, Source};

impl Repos {
    pub(crate) async fn creation_worktrees(
        &self,
        root: &Path,
    ) -> Result<HashMap<String, String>, EngineError> {
        let output = self
            .git(&["worktree", "list", "--porcelain", "-z"], Some(root))
            .await?;
        let mut path = None;
        let mut result = HashMap::new();
        for field in output.split('\0') {
            if let Some(value) = field.strip_prefix("worktree ") {
                path = Some(value.to_owned());
            }
            if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                if let Some(path) = path.take() {
                    result.insert(branch.into(), path);
                }
            }
        }
        Ok(result)
    }

    pub(crate) async fn creation_branches(&self, root: &Path) -> Result<Vec<Row>, EngineError> {
        let current = self.current_branch(root).await?;
        let worktrees = self.creation_worktrees(root).await?;
        let refs = self
            .git(
                &[
                    "for-each-ref",
                    "--sort=-committerdate",
                    "--format=%(refname)%00%(symref)",
                    "refs/heads/",
                    "refs/remotes/",
                ],
                Some(root),
            )
            .await?;
        let locals: HashSet<_> = refs
            .lines()
            .filter_map(|line| line.split('\0').next()?.strip_prefix("refs/heads/"))
            .collect();
        let mut rows = vec![];
        for line in refs.lines() {
            let (reference, symbolic) = line.split_once('\0').unwrap_or((line, ""));
            if !symbolic.is_empty() {
                continue;
            }
            let (title, remote) = if let Some(name) = reference.strip_prefix("refs/heads/") {
                (name, None)
            } else if let Some((remote, name)) = reference
                .strip_prefix("refs/remotes/")
                .and_then(|s| s.split_once('/'))
            {
                if locals.contains(name) {
                    continue;
                }
                (name, Some(remote))
            } else {
                continue;
            };
            if title == current {
                continue;
            }
            rows.push(Row {
                source: Source::Branch {
                    reference: reference.into(),
                },
                title: title.into(),
                lead: String::new(),
                branch: Some(title.into()),
                author: remote.map(str::to_owned),
                in_use: worktrees.contains_key(title),
            });
        }
        Ok(rows)
    }

    pub(crate) async fn prepare_creation_source(
        &self,
        root: &Path,
        source: &CheckoutSource,
        github: &crate::source_control::GitHubCli,
    ) -> Result<(Worktree, bool), EngineError> {
        let request_id = uuid::Uuid::parse_str(&source.request_id)
            .map_err(|_| EngineError::Other("Invalid source request ID".into()))?;
        let (branch, reference, pr) = match &source.source {
            Source::Branch { reference } => {
                let name = reference
                    .strip_prefix("refs/heads/")
                    .or_else(|| {
                        reference
                            .strip_prefix("refs/remotes/")
                            .and_then(|r| r.split_once('/').map(|(_, n)| n))
                    })
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| EngineError::Other("Choose a local or remote branch".into()))?;
                self.git(&["show-ref", "--verify", reference], Some(root))
                    .await?;
                (name.to_owned(), Some(reference.as_str()), None)
            }
            Source::Pr { number } if *number > 0 => (
                github
                    .creation_pr_branch(root, *number)
                    .await
                    .map_err(|e| EngineError::Other(e.to_string()))?,
                None,
                Some(*number),
            ),
            _ => return Err(EngineError::Other("Choose a branch or pull request".into())),
        };
        self.git(&["check-ref-format", "--branch", &branch], Some(root))
            .await?;
        let existing = self.creation_worktrees(root).await?;
        let reused = existing.get(&branch).map(PathBuf::from);
        let name = format!("source-{}", request_id.simple());
        let repo_name = root.file_name().unwrap_or_default();
        let base = self.inner.worktrees.root().join(repo_name);
        let path = reused.clone().unwrap_or_else(|| base.join(&name));
        let created = reused.is_none();
        if created {
            std::fs::create_dir_all(&base)?;
            if std::fs::canonicalize(&base)?.starts_with(std::fs::canonicalize(root)?) {
                return Err(EngineError::Other(
                    "Choose a worktree location outside the project in Settings".into(),
                ));
            }
            if path.exists() {
                return Err(EngineError::Other(format!(
                    "A previous source checkout exists at {}. Open it or choose the branch again.",
                    path.display()
                )));
            }
            let destination = path.to_string_lossy();
            if let Some(number) = pr {
                self.git(
                    &["worktree", "add", "--detach", &destination, "HEAD"],
                    Some(root),
                )
                .await?;
                if let Err(err) = github.checkout_creation_pr(&path, number, &branch).await {
                    let cleanup = self
                        .git(&["worktree", "remove", &destination], Some(root))
                        .await;
                    return Err(EngineError::Other(if cleanup.is_ok() {
                        err.to_string()
                    } else {
                        format!("{err}. The checkout is retained at {}", path.display())
                    }));
                }
            } else if self
                .git(
                    &["show-ref", "--verify", &format!("refs/heads/{branch}")],
                    Some(root),
                )
                .await
                .is_ok()
            {
                self.git(&["worktree", "add", &destination, &branch], Some(root))
                    .await?;
            } else {
                self.git(
                    &[
                        "worktree",
                        "add",
                        "--track",
                        "-b",
                        &branch,
                        &destination,
                        reference.unwrap(),
                    ],
                    Some(root),
                )
                .await?;
            }
            crate::archives::Archives::new(self.data_dir())
                .register_managed(root, &path)
                .await?;
        }
        let checkout = self.checkout_identity(&path).await?;
        let branch = self.current_branch(&path).await?;
        Ok((
            Worktree {
                repo_path: root.to_string_lossy().into(),
                path: path.to_string_lossy().into(),
                branch,
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                checkout_id: Some(checkout.id),
            },
            created,
        ))
    }
}
