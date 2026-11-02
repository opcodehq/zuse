//! Authenticated discovery and checkout for the new-chat source picker.
use super::*;
use serde_json::Value;
use zuse_protocol::creation_source::{Row, Source, Tab};

impl GitHubCli {
    async fn source_command(
        &self,
        cwd: &Path,
        args: Vec<String>,
        checkout: bool,
    ) -> Result<Value, ChangeRequestError> {
        let output = self
            .runner
            .run(ProcessRequest {
                args,
                cwd: Some(cwd.to_owned()),
                timeout: if checkout {
                    Duration::from_secs(120)
                } else {
                    GITHUB_TIMEOUT
                },
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
        if checkout {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)
    }

    pub(crate) async fn creation_sources(
        &self,
        cwd: &Path,
        tab: Tab,
    ) -> Result<Vec<Row>, ChangeRequestError> {
        let (command, fields) = match tab {
            Tab::PullRequests => ("pr", "number,title,headRefName,author"),
            Tab::Issues => ("issue", "number,title,author"),
            Tab::Branches => return Ok(vec![]),
        };
        let result = self
            .source_command(
                cwd,
                [
                    command, "list", "--state", "open", "--limit", "100", "--json", fields,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
                false,
            )
            .await?;
        result
            .as_array()
            .ok_or(ChangeRequestError::Decode)?
            .iter()
            .map(|v| {
                let number = v["number"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or(ChangeRequestError::Decode)?;
                Ok(Row {
                    source: if tab == Tab::Issues {
                        Source::Issue { number }
                    } else {
                        Source::Pr { number }
                    },
                    lead: format!("#{number}"),
                    title: v["title"]
                        .as_str()
                        .ok_or(ChangeRequestError::Decode)?
                        .into(),
                    branch: v["headRefName"].as_str().map(str::to_owned),
                    author: v["author"]["login"].as_str().map(str::to_owned),
                    in_use: false,
                })
            })
            .collect()
    }

    pub(crate) async fn creation_pr_branch(
        &self,
        cwd: &Path,
        number: u64,
    ) -> Result<String, ChangeRequestError> {
        let value = self
            .source_command(
                cwd,
                vec![
                    "pr".into(),
                    "view".into(),
                    number.to_string(),
                    "--json".into(),
                    "headRefName,baseRefName,isCrossRepository,headRepositoryOwner,headRepository"
                        .into(),
                ],
                false,
            )
            .await?;
        let head = value["headRefName"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or(ChangeRequestError::Decode)?;
        // A fork's main branch must not accidentally reuse the base checkout.
        let collision = value["isCrossRepository"] == true
            && (value["baseRefName"].as_str() == Some(head)
                || value["headRepository"]["defaultBranchRef"]["name"].as_str() == Some(head));
        Ok(if collision {
            format!(
                "{}/{head}",
                value["headRepositoryOwner"]["login"]
                    .as_str()
                    .unwrap_or("fork")
            )
        } else {
            head.into()
        })
    }

    pub(crate) async fn creation_cloud_pr(
        &self,
        cwd: &Path,
        number: u64,
    ) -> Result<zuse_protocol::creation_source::Resolution, ChangeRequestError> {
        let value = self
            .source_command(
                cwd,
                vec![
                    "pr".into(),
                    "view".into(),
                    number.to_string(),
                    "--json".into(),
                    "headRefName,isCrossRepository".into(),
                ],
                false,
            )
            .await?;
        let head = value["headRefName"]
            .as_str()
            .filter(|head| !head.is_empty())
            .ok_or(ChangeRequestError::Decode)?;
        let fork = value["isCrossRepository"]
            .as_bool()
            .ok_or(ChangeRequestError::Decode)?;
        if !fork
            && !head
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
        {
            return Err(ChangeRequestError::Decode);
        }
        Ok(zuse_protocol::creation_source::Resolution::CloudRef {
            label: format!("PR #{number} · {head}"),
            base_ref: if fork {
                format!("refs/pull/{number}/head")
            } else {
                format!("origin/{head}")
            },
            branch: (!fork).then(|| head.into()),
        })
    }

    pub(crate) async fn checkout_creation_pr(
        &self,
        cwd: &Path,
        number: u64,
        branch: &str,
    ) -> Result<(), ChangeRequestError> {
        self.source_command(
            cwd,
            vec![
                "pr".into(),
                "checkout".into(),
                number.to_string(),
                "--branch".into(),
                branch.into(),
            ],
            true,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn creation_issue(
        &self,
        cwd: &Path,
        number: u64,
    ) -> Result<(String, String), ChangeRequestError> {
        let value = self
            .source_command(
                cwd,
                vec![
                    "issue".into(),
                    "view".into(),
                    number.to_string(),
                    "--json".into(),
                    "number,title,body,url".into(),
                ],
                false,
            )
            .await?;
        if value["number"].as_u64() != Some(number) {
            return Err(ChangeRequestError::Decode);
        }
        let title = value["title"]
            .as_str()
            .ok_or(ChangeRequestError::Decode)?
            .to_owned();
        let markdown = format!(
            "# #{number}: {title}\n\n{}\n\n{}\n",
            value["url"].as_str().unwrap_or_default(),
            value["body"].as_str().unwrap_or_default()
        );
        Ok((title, markdown))
    }
}
