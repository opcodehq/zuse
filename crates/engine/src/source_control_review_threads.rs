//! Inline feedback is a separate, doubly paginated GitHub connection.
use super::*;
use serde_json::{Value, json};
use zuse_protocol::{ChangeRequestComment, ChangeRequestMergeMethod, ChangeRequestReviewThread};

const COMMENT_FIELDS: &str = "id url body createdAt diffHunk author { login avatarUrl(size: 64) } viewerDidAuthor replyTo { id }";
const PAGE_INFO: &str = "pageInfo { hasNextPage endCursor }";

impl GitHubCli {
    async fn review_query(&self, query: String, variables: Value, budget: &mut usize) -> PrResult {
        let _slot = self
            .read_slots
            .acquire()
            .await
            .map_err(|_| ChangeRequestError::CommandFailed)?;
        let output = self
            .runner
            .run(ProcessRequest {
                args: vec!["api".into(), "graphql".into(), "--input".into(), "-".into()],
                stdin: Some(
                    serde_json::to_vec(&json!({"query": query, "variables": variables}))
                        .map_err(|_| ChangeRequestError::Decode)?,
                ),
                ..github_request()
            })
            .await
            .map_err(classify_run_error)?;
        if !output.success {
            return Err(classify_github_failure(&output.stderr));
        }
        *budget = budget
            .checked_sub(output.stdout.len())
            .ok_or(ChangeRequestError::Decode)?;
        if output.stdout_truncated {
            return Err(ChangeRequestError::Decode);
        }
        let mut value: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| ChangeRequestError::Decode)?;
        if value.get("errors").is_some() {
            return Err(ChangeRequestError::Decode);
        }
        // Deleted actors and nullable locations use the protocol defaults.
        remove_nulls(&mut value);
        Ok(value)
    }

    pub(crate) async fn fetch_review_threads(
        &self,
        url: &str,
    ) -> Result<Vec<ChangeRequestReviewThread>, ChangeRequestError> {
        Ok(self.fetch_review_threads_and_merge_methods(url).await?.0)
    }

    /// The repository's allowed merge methods ride along on the first page.
    pub(crate) async fn fetch_review_threads_and_merge_methods(
        &self,
        url: &str,
    ) -> Result<
        (
            Vec<ChangeRequestReviewThread>,
            Option<Vec<ChangeRequestMergeMethod>>,
        ),
        ChangeRequestError,
    > {
        let url = validated_pull_request_url(url)?;
        let parts: Vec<_> = url.split('/').collect();
        let number: u64 = parts[6].parse().map_err(|_| ChangeRequestError::Decode)?;
        let query = format!(
            "query($owner:String!,$name:String!,$number:Int!,$after:String) {{ repository(owner:$owner,name:$name) {{ squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed pullRequest(number:$number) {{ reviewThreads(first:100,after:$after) {{ {PAGE_INFO} nodes {{ id isResolved isOutdated path line startLine originalLine originalStartLine diffSide startDiffSide comments(first:100) {{ {PAGE_INFO} nodes {{ {COMMENT_FIELDS} }} }} }} }} }} }} }}"
        );
        let replies_query = format!(
            "query($id:ID!,$after:String) {{ node(id:$id) {{ ... on PullRequestReviewThread {{ comments(first:100,after:$after) {{ {PAGE_INFO} nodes {{ {COMMENT_FIELDS} }} }} }} }} }}"
        );
        let mut threads = Vec::new();
        let mut merge_methods = None;
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        // Bound aggregate memory; never present a partial result as complete.
        let mut budget = 8 * GITHUB_OUTPUT_LIMIT;
        loop {
            let value = self
                .review_query(
                    query.clone(),
                    json!({
                        "owner": parts[3], "name": parts[4], "number": number, "after": cursor,
                    }),
                    &mut budget,
                )
                .await?;
            if cursor.is_none() {
                merge_methods = allowed_merge_methods(&value["data"]["repository"]);
            }
            let connection = &value["data"]["repository"]["pullRequest"]["reviewThreads"];
            for node in connection["nodes"]
                .as_array()
                .ok_or(ChangeRequestError::Decode)?
            {
                let mut thread_value = node.clone();
                let mut comments = node["comments"].clone();
                thread_value["comments"] = json!([]);
                let mut thread: ChangeRequestReviewThread =
                    serde_json::from_value(thread_value).map_err(|_| ChangeRequestError::Decode)?;
                let mut seen_comments = std::collections::HashSet::new();
                loop {
                    let page: Vec<ChangeRequestComment> =
                        serde_json::from_value(comments["nodes"].clone())
                            .map_err(|_| ChangeRequestError::Decode)?;
                    thread.comments.extend(page);
                    let Some(after) = next_cursor(&comments, &mut seen_comments)? else {
                        break;
                    };
                    let value = self
                        .review_query(
                            replies_query.clone(),
                            json!({"id": thread.id, "after": after}),
                            &mut budget,
                        )
                        .await?;
                    comments = value["data"]["node"]["comments"].clone();
                }
                threads.push(thread);
            }
            cursor = next_cursor(connection, &mut seen)?;
            if cursor.is_none() {
                break;
            }
        }
        Ok((threads, merge_methods))
    }
}

fn allowed_merge_methods(repository: &Value) -> Option<Vec<ChangeRequestMergeMethod>> {
    let flags = [
        ("squashMergeAllowed", ChangeRequestMergeMethod::Squash),
        ("mergeCommitAllowed", ChangeRequestMergeMethod::Merge),
        ("rebaseMergeAllowed", ChangeRequestMergeMethod::Rebase),
    ];
    if flags
        .iter()
        .all(|(key, _)| repository[*key].as_bool().is_none())
    {
        return None;
    }
    Some(
        flags
            .into_iter()
            .filter(|(key, _)| repository[*key].as_bool() == Some(true))
            .map(|(_, method)| method)
            .collect(),
    )
}

fn next_cursor(
    connection: &Value,
    seen: &mut std::collections::HashSet<String>,
) -> Result<Option<String>, ChangeRequestError> {
    match connection["pageInfo"]["hasNextPage"].as_bool() {
        Some(false) => Ok(None),
        Some(true) => {
            let cursor = connection["pageInfo"]["endCursor"]
                .as_str()
                .ok_or(ChangeRequestError::Decode)?;
            if !valid_page_cursor(cursor) || !seen.insert(cursor.to_owned()) {
                return Err(ChangeRequestError::Decode);
            }
            Ok(Some(cursor.to_owned()))
        }
        None => Err(ChangeRequestError::Decode),
    }
}
