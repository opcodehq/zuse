//! Explicit PR creation uses JSON stdin and checks the published source revision.
use super::*;
use serde_json::{Value, json};
impl GitHubCli {
    async fn creation_read(&self, endpoint: String) -> Result<Value, ChangeRequestError> {
        let result = self
            .runner
            .run(ProcessRequest {
                args: vec!["api".into(), endpoint],
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !result.success {
            return Err(classify_github_failure(&result.stderr));
        }
        if result.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| ChangeRequestError::Decode)
    }
    pub(crate) async fn create_pull_request(
        &self,
        head_repository: &str,
        head_branch: &str,
        head_sha: &str,
        title: &str,
        body: &str,
        base: &str,
        base_repository: Option<&str>,
        draft: bool,
    ) -> Result<String, ChangeRequestError> {
        if !valid_pr_repository(head_repository)
            || base_repository.is_some_and(|r| !valid_pr_repository(r))
            || title.trim().is_empty()
            || title.chars().count() > 256
            || body.len() > 60000
            || [title, body, base, head_branch]
                .iter()
                .any(|v| v.as_bytes().contains(&0))
            || !zuse_protocol::change_request_assessment::valid_head_oid(head_sha)
            || head_branch.is_empty()
            || head_branch.len() > 255
            || base.len() > 255
        {
            return Err(ChangeRequestError::Decode);
        }
        let head_info = self
            .creation_read(format!("repos/{head_repository}"))
            .await?;
        let head_repo = head_info["full_name"]
            .as_str()
            .filter(|r| valid_pr_repository(r))
            .ok_or(ChangeRequestError::Decode)?;
        let target = base_repository
            .or_else(|| head_info["parent"]["full_name"].as_str())
            .unwrap_or(head_repo);
        if !valid_pr_repository(target) {
            return Err(ChangeRequestError::Decode);
        }
        let target_info = if target.eq_ignore_ascii_case(head_repo) {
            head_info.clone()
        } else {
            self.creation_read(format!("repos/{target}")).await?
        };
        let target_repo = target_info["full_name"]
            .as_str()
            .filter(|r| valid_pr_repository(r))
            .ok_or(ChangeRequestError::Decode)?;
        let base = if base.trim().is_empty() {
            target_info["default_branch"]
                .as_str()
                .filter(|b| !b.is_empty())
                .ok_or(ChangeRequestError::Decode)?
        } else {
            base
        };
        let mut branch_url = reqwest::Url::parse(&format!(
            "https://api.github.com/repos/{head_repo}/branches/"
        ))
        .map_err(|_| ChangeRequestError::Decode)?;
        branch_url
            .path_segments_mut()
            .map_err(|_| ChangeRequestError::Decode)?
            .pop_if_empty()
            .push(head_branch);
        let branch = self.creation_read(branch_url.to_string()).await?;
        if branch["commit"]["sha"].as_str() != Some(head_sha) {
            return Err(ChangeRequestError::RepositoryUnavailable);
        }
        let cross = !head_repo.eq_ignore_ascii_case(target_repo);
        let mut payload = json!({"title":title,"body":body,"base":base,"head":if cross{format!("{}:{head_branch}",head_repo.split('/').next().unwrap())}else{head_branch.into()},"draft":draft});
        if cross {
            payload["head_repo"] = json!(head_repo.split('/').nth(1).unwrap());
        }
        let result = self
            .runner
            .run(ProcessRequest {
                args: vec![
                    "api".into(),
                    "--method".into(),
                    "POST".into(),
                    format!("repos/{target_repo}/pulls"),
                    "--input".into(),
                    "-".into(),
                ],
                stdin: Some(serde_json::to_vec(&payload).map_err(|_| ChangeRequestError::Decode)?),
                ..github_request()
            })
            .await;
        // The attempt may have succeeded despite an unreadable receipt. Do not
        // leave an older detail read available as evidence that nothing happened.
        let mut cache = self.pr_cache.lock().await;
        cache.entries.clear();
        cache.in_flight.clear();
        drop(cache);
        let result = result.map_err(classify_run_error)?;
        if !result.success {
            return Err(classify_github_failure(&result.stderr));
        }
        if result.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let result: Value =
            serde_json::from_slice(&result.stdout).map_err(|_| ChangeRequestError::Decode)?;
        let url = validated_pull_request_url(
            result["html_url"]
                .as_str()
                .ok_or(ChangeRequestError::Decode)?,
        )?;
        let expected = format!("https://github.com/{target_repo}/pull/");
        if !url
            .to_ascii_lowercase()
            .starts_with(&expected.to_ascii_lowercase())
            || result["head"]["sha"].as_str() != Some(head_sha)
        {
            return Err(ChangeRequestError::Decode);
        }
        Ok(url)
    }
}
