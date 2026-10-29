//! Bounded CI reads share the source-control process runner and its deadlines.
use super::*;
use zuse_protocol::{
    ChangeRequestDetail,
    change_request_assessment::{check_failed, check_status},
};

impl GitHubCli {
    pub(crate) async fn ci_snapshot(
        &self,
        url: &str,
    ) -> Result<ChangeRequestDetail, ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        let output = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "pr".into(),
                    "view".into(),
                    url,
                    "--json".into(),
                    "url,number,state,isDraft,headRefName,headRefOid,headRepository,statusCheckRollup".into(),
                ],
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        let reported = value.get("statusCheckRollup").is_some_and(|v| !v.is_null());
        remove_nulls(&mut value);
        let mut detail: ChangeRequestDetail =
            serde_json::from_value(value).map_err(|_| ChangeRequestError::Decode)?;
        if reported {
            detail.ci = zuse_protocol::change_request_assessment::ChangeRequestCi::from_checks(
                &detail.status_check_rollup,
            );
        }
        Ok(detail)
    }

    /// Require the exact checked-out commit. No checkout, fetch or push is performed.
    pub(crate) async fn ci_checkout_matches(
        &self,
        cwd: &Path,
        branch: &str,
        sha: &str,
        repository: &str,
    ) -> bool {
        let git = GitCheckoutInspector::new(self.runner.clone());
        if git.git(cwd, &["branch", "--show-current"]).await.as_deref() != Some(branch)
            || git.git(cwd, &["rev-parse", "HEAD"]).await.as_deref() != Some(sha)
            || git.git(cwd, &["status", "--porcelain"]).await.as_deref() != Some("")
        {
            return false;
        }
        let push_key = format!("branch.{branch}.pushRemote");
        let remote_key = format!("branch.{branch}.remote");
        let remote = match git.git(cwd, &["config", "--get", &push_key]).await {
            Some(remote) => remote,
            None => match git
                .git(cwd, &["config", "--get", "remote.pushDefault"])
                .await
            {
                Some(remote) => remote,
                None => git
                    .git(cwd, &["config", "--get", &remote_key])
                    .await
                    .unwrap_or_else(|| "origin".into()),
            },
        };
        let Some(urls) = git
            .git(cwd, &["remote", "get-url", "--push", "--all", &remote])
            .await
        else {
            return false;
        };
        // A normal push can publish to multiple push URLs. Admit one exact
        // GitHub source repository only, including fork PRs.
        if urls.lines().count() != 1 {
            return false;
        }
        parse_git_remote(&urls).is_some_and(|remote| {
            remote.host == "github.com"
                && format!("{}/{}", remote.owner, remote.repository)
                    .eq_ignore_ascii_case(repository)
        })
    }

    /// Failed logs are untrusted context. Never execute URLs or interpolate shell text.
    pub(crate) async fn ci_failure_logs(
        &self,
        detail: &ChangeRequestDetail,
    ) -> Result<String, ChangeRequestError> {
        let canonical = validated_pull_request_url(&detail.url)?;
        let parsed = reqwest::Url::parse(&canonical).map_err(|_| ChangeRequestError::Decode)?;
        let parts: Vec<_> = parsed.path().trim_matches('/').split('/').collect();
        let repository = format!("{}/{}", parts[0], parts[1]);
        let mut logs = String::new();
        let failures: Vec<_> = detail
            .status_check_rollup
            .iter()
            .filter(|c| check_failed(&check_status(c)))
            .collect();
        for check in failures.iter().take(8) {
            let name = if check.name.is_empty() {
                &check.context
            } else {
                &check.name
            };
            logs.push_str(&format!("\nCheck: {name}\n"));
            let link = if check.details_url.is_empty() {
                &check.target_url
            } else {
                &check.details_url
            };
            let target = action_ids(link, &repository)
                .map(|ids| (repository.clone(), ids))
                .or_else(|| {
                    let head = detail.head_repository.as_ref()?;
                    if !valid_pr_repository(&head.name_with_owner) {
                        return None;
                    }
                    action_ids(link, &head.name_with_owner)
                        .map(|ids| (head.name_with_owner.clone(), ids))
                });
            let Some((log_repository, (run, job))) = target else {
                logs.push_str(
                    "No GitHub Actions log is available; inspect this check's provider manually.\n",
                );
                continue;
            };
            let mut args = vec![
                "run".into(),
                "view".into(),
                run,
                "--repo".into(),
                log_repository,
                "--log-failed".into(),
            ];
            if let Some(job) = job {
                args.extend(["--job".into(), job]);
            }
            let output = self
                .runner
                .run(ProcessRequest {
                    args,
                    output_limit: 48 * 1024,
                    ..github_request()
                })
                .await
                .map_err(classify_run_error)?;
            if !output.success {
                return Err(classify_github_failure(&output.stderr));
            }
            logs.push_str(&String::from_utf8_lossy(&output.stdout));
            if output.stdout_truncated {
                logs.push_str("\n[Log truncated at 48 KiB]\n");
            }
        }
        if failures.len() > 8 {
            logs.push_str(
                "\n[Additional failures omitted; retrieve remaining logs before fixing.]\n",
            );
        }
        Ok(logs)
    }
}

fn action_ids(link: &str, repository: &str) -> Option<(String, Option<String>)> {
    let url = reqwest::Url::parse(link).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
    if parts.len() != 5 && parts.len() != 7 {
        return None;
    }
    if format!("{}/{}", parts[0], parts[1]) != repository || parts[2..4] != ["actions", "runs"] {
        return None;
    }
    let numeric = |v: &str| !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit());
    if !numeric(parts[4]) {
        return None;
    }
    let job = if parts.len() == 7 {
        if parts[5] != "job" || !numeric(parts[6]) {
            return None;
        }
        Some(parts[6].to_owned())
    } else {
        None
    };
    Some((parts[4].to_owned(), job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct ScriptedRunner {
        requests: Mutex<Vec<ProcessRequest>>,
        responses: Mutex<std::collections::VecDeque<ProcessOutput>>,
    }
    #[async_trait]
    impl ProcessRunner for ScriptedRunner {
        async fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, ProcessRunError> {
            self.requests.lock().unwrap().push(request);
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected process"))
        }
    }
    #[test]
    fn action_links_are_scoped_and_numeric() {
        assert_eq!(
            action_ids("https://github.com/a/b/actions/runs/42/job/9", "a/b"),
            Some(("42".into(), Some("9".into())))
        );
        for bad in [
            "https://evil.test/a/b/actions/runs/42",
            "https://github.com/a/c/actions/runs/42",
            "https://github.com/a/b/actions/runs/--help",
            "https://user@github.com/a/b/actions/runs/42",
            "https://github.com/a/b/actions/runs/42/job/9/more",
        ] {
            assert!(action_ids(bad, "a/b").is_none());
        }
    }
    #[tokio::test]
    async fn failed_job_logs_use_literal_arguments_and_bounds() {
        let runner = Arc::new(ScriptedRunner {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(
                [ProcessOutput {
                    success: true,
                    stdout: b"failed assertion".to_vec(),
                    stderr: Vec::new(),
                    stdout_truncated: false,
                }]
                .into(),
            ),
        });
        let github = GitHubCli {
            runner: runner.clone(),
            ..GitHubCli::new()
        };
        let detail = ChangeRequestDetail {
            url: "https://github.com/a/b/pull/1".into(),
            status_check_rollup: vec![zuse_protocol::ChangeRequestCheck {
                name: "test".into(),
                conclusion: "FAILURE".into(),
                details_url: "https://github.com/a/b/actions/runs/42/job/9".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            github
                .ci_failure_logs(&detail)
                .await
                .unwrap()
                .contains("failed assertion")
        );
        let calls = runner.requests.lock().unwrap();
        assert_eq!(
            calls[0].args,
            [
                "run",
                "view",
                "42",
                "--repo",
                "a/b",
                "--log-failed",
                "--job",
                "9"
            ]
        );
        assert_eq!(calls[0].output_limit, 48 * 1024);
    }
    #[tokio::test]
    async fn automatic_repairs_require_one_matching_push_repository() {
        let output = |value: &str| ProcessOutput {
            success: true,
            stdout: value.as_bytes().to_vec(),
            stderr: vec![],
            stdout_truncated: false,
        };
        for (urls, allowed) in [
            ("git@github.com:example/project.git", true),
            ("git@github.com:other/project.git", false),
            (
                "git@github.com:example/project.git\ngit@github.com:other/project.git",
                false,
            ),
        ] {
            let runner = Arc::new(ScriptedRunner {
                requests: Mutex::new(vec![]),
                responses: Mutex::new(
                    [
                        output("fix-ci"),
                        output(&"a".repeat(40)),
                        output(""),
                        output("origin"),
                        output(urls),
                    ]
                    .into(),
                ),
            });
            let github = GitHubCli {
                runner: runner.clone(),
                ..GitHubCli::new()
            };
            assert_eq!(
                github
                    .ci_checkout_matches(
                        Path::new("/checkout"),
                        "fix-ci",
                        &"a".repeat(40),
                        "example/project"
                    )
                    .await,
                allowed
            );
            let calls = runner.requests.lock().unwrap();
            assert_eq!(
                calls[4].args,
                ["remote", "get-url", "--push", "--all", "origin"]
            );
            assert!(calls.iter().all(|request| {
                request.program == "git"
                    && !request
                        .args
                        .iter()
                        .any(|arg| arg == "push" || arg == "fetch")
            }));
        }
    }
    #[tokio::test]
    async fn failed_job_logs_accept_the_declared_fork_source_repository() {
        let runner = Arc::new(ScriptedRunner {
            requests: Mutex::new(vec![]),
            responses: Mutex::new(
                [ProcessOutput {
                    success: true,
                    stdout: b"fork failure".to_vec(),
                    stderr: vec![],
                    stdout_truncated: false,
                }]
                .into(),
            ),
        });
        let github = GitHubCli {
            runner: runner.clone(),
            ..GitHubCli::new()
        };
        let detail = ChangeRequestDetail {
            url: "https://github.com/base/project/pull/1".into(),
            head_repository: Some(zuse_protocol::ChangeRequestHeadRepository {
                name_with_owner: "fork/project".into(),
            }),
            status_check_rollup: vec![zuse_protocol::ChangeRequestCheck {
                context: "fork test".into(),
                conclusion: "FAILURE".into(),
                target_url: "https://github.com/fork/project/actions/runs/17/job/19".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let logs = github.ci_failure_logs(&detail).await.unwrap();
        assert!(logs.contains("Check: fork test"));
        assert!(logs.contains("fork failure"));
        assert_eq!(
            runner.requests.lock().unwrap()[0].args,
            [
                "run",
                "view",
                "17",
                "--repo",
                "fork/project",
                "--log-failed",
                "--job",
                "19"
            ]
        );
    }
}
