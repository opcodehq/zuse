//! Inline feedback is a separate, doubly paginated GitHub connection.
use super::*;
use serde_json::{Value, json};
use zuse_protocol::{
    ChangeRequestActor, ChangeRequestComment, ChangeRequestDetail, ChangeRequestMergeMethod,
    ChangeRequestReviewThread, ChangeRequestTimelineCommit, ChangeRequestTimelineItem,
    ChangeRequestTimelineSubject,
};

const COMMENT_FIELDS: &str = "id url body createdAt diffHunk author { login avatarUrl(size: 64) } viewerDidAuthor replyTo { id }";
const PAGE_INFO: &str = "pageInfo { hasNextPage endCursor }";
const ACTOR: &str = "actor { login avatarUrl(size: 64) }";
const SUBJECT: &str = "__typename ... on Issue { number title url state } ... on PullRequest { number title url state }";
const REVIEWER: &str =
    "requestedReviewer { ... on User { login } ... on Bot { login } ... on Team { name } }";

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
        Ok(self.fetch_review_threads_page(url, false).await?.0)
    }

    /// With `detail`, the first page also carries the repository's merge
    /// settings and the pull request's timeline, returned as the first page's
    /// `repository` value for [`apply_detail_extras`].
    pub(crate) async fn fetch_review_threads_page(
        &self,
        url: &str,
        detail: bool,
    ) -> Result<(Vec<ChangeRequestReviewThread>, Option<Value>), ChangeRequestError> {
        let url = validated_pull_request_url(url)?;
        let parts: Vec<_> = url.split('/').collect();
        let number: u64 = parts[6].parse().map_err(|_| ChangeRequestError::Decode)?;
        let threads_query = |repository_extras: &str, pull_request_extras: &str| {
            format!(
                "query($owner:String!,$name:String!,$number:Int!,$after:String) {{ repository(owner:$owner,name:$name) {{ {repository_extras}pullRequest(number:$number) {{ {pull_request_extras}reviewThreads(first:100,after:$after) {{ {PAGE_INFO} nodes {{ id isResolved isOutdated path line startLine originalLine originalStartLine diffSide startDiffSide comments(first:100) {{ {PAGE_INFO} nodes {{ {COMMENT_FIELDS} }} }} }} }} }} }} }}"
            )
        };
        let query = threads_query("", "");
        let first_query = if detail {
            threads_query(
                "squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed ",
                &format!(
                    "mergeStateStatus reviewRequests(first:20) {{ nodes {{ {REVIEWER} }} }} closingIssuesReferences(first:10) {{ nodes {{ number title url state }} }} timelineItems(last:100, itemTypes:[PULL_REQUEST_COMMIT, HEAD_REF_FORCE_PUSHED_EVENT, PULL_REQUEST_REVIEW, ISSUE_COMMENT, REVIEW_REQUESTED_EVENT, REVIEW_REQUEST_REMOVED_EVENT, LABELED_EVENT, UNLABELED_EVENT, READY_FOR_REVIEW_EVENT, CONVERT_TO_DRAFT_EVENT, BASE_REF_CHANGED_EVENT, AUTO_MERGE_ENABLED_EVENT, AUTO_MERGE_DISABLED_EVENT, MERGED_EVENT, CLOSED_EVENT, REOPENED_EVENT, CROSS_REFERENCED_EVENT, CONNECTED_EVENT, DISCONNECTED_EVENT]) {{ totalCount nodes {{ __typename \
                     ... on PullRequestCommit {{ commit {{ oid messageHeadline committedDate author {{ name user {{ login avatarUrl(size: 64) }} }} statusCheckRollup {{ state }} }} }} \
                     ... on HeadRefForcePushedEvent {{ {ACTOR} createdAt beforeCommit {{ oid }} afterCommit {{ oid }} }} \
                     ... on PullRequestReview {{ author {{ login avatarUrl(size: 64) }} createdAt submittedAt url state }} \
                     ... on IssueComment {{ author {{ login avatarUrl(size: 64) }} createdAt url }} \
                     ... on ReviewRequestedEvent {{ {ACTOR} createdAt {REVIEWER} }} \
                     ... on ReviewRequestRemovedEvent {{ {ACTOR} createdAt {REVIEWER} }} \
                     ... on LabeledEvent {{ {ACTOR} createdAt label {{ name color }} }} \
                     ... on UnlabeledEvent {{ {ACTOR} createdAt label {{ name color }} }} \
                     ... on ReadyForReviewEvent {{ {ACTOR} createdAt }} \
                     ... on ConvertToDraftEvent {{ {ACTOR} createdAt }} \
                     ... on BaseRefChangedEvent {{ {ACTOR} createdAt previousRefName currentRefName }} \
                     ... on AutoMergeEnabledEvent {{ {ACTOR} createdAt }} \
                     ... on AutoMergeDisabledEvent {{ {ACTOR} createdAt }} \
                     ... on MergedEvent {{ {ACTOR} createdAt commit {{ oid }} }} \
                     ... on ClosedEvent {{ {ACTOR} createdAt }} \
                     ... on ReopenedEvent {{ {ACTOR} createdAt }} \
                     ... on CrossReferencedEvent {{ {ACTOR} createdAt source {{ {SUBJECT} }} }} \
                     ... on ConnectedEvent {{ {ACTOR} createdAt subject {{ {SUBJECT} }} }} \
                     ... on DisconnectedEvent {{ {ACTOR} createdAt subject {{ {SUBJECT} }} }} \
                     }} }} "
                ),
            )
        } else {
            query.clone()
        };
        let replies_query = format!(
            "query($id:ID!,$after:String) {{ node(id:$id) {{ ... on PullRequestReviewThread {{ comments(first:100,after:$after) {{ {PAGE_INFO} nodes {{ {COMMENT_FIELDS} }} }} }} }} }}"
        );
        let mut threads = Vec::new();
        let mut first_page = None;
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        // Bound aggregate memory; never present a partial result as complete.
        let mut budget = 8 * GITHUB_OUTPUT_LIMIT;
        loop {
            let value = self
                .review_query(
                    if cursor.is_none() {
                        first_query.clone()
                    } else {
                        query.clone()
                    },
                    json!({
                        "owner": parts[3], "name": parts[4], "number": number, "after": cursor,
                    }),
                    &mut budget,
                )
                .await?;
            if detail && cursor.is_none() {
                first_page = Some(value["data"]["repository"].clone());
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
        Ok((threads, first_page))
    }
}

/// Fill the detail fields the first review-threads page carries. Each field
/// stays None when GitHub left it out.
pub(crate) fn apply_detail_extras(detail: &mut ChangeRequestDetail, repository: &Value) {
    detail.merge_methods = allowed_merge_methods(repository);
    let pull_request = &repository["pullRequest"];
    detail.merge_state_status = text(&pull_request["mergeStateStatus"]);
    detail.requested_reviewers = pull_request["reviewRequests"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| reviewer_name(&node["requestedReviewer"]))
                .collect()
        });
    detail.linked_issues = pull_request["closingIssuesReferences"]["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(|node| subject(node, false)).collect());
    let timeline = &pull_request["timelineItems"];
    detail.timeline_total = timeline["totalCount"].as_u64();
    detail.timeline = timeline["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().filter_map(timeline_item).collect());
}

fn timeline_item(node: &Value) -> Option<ChangeRequestTimelineItem> {
    let kind = match node["__typename"].as_str()? {
        "PullRequestCommit" => {
            let commit = &node["commit"];
            return Some(ChangeRequestTimelineItem {
                kind: "commit".into(),
                actor: actor(&commit["author"]["user"]),
                created_at: text(&commit["committedDate"]).unwrap_or_default(),
                state: text(&commit["statusCheckRollup"]["state"]),
                commit: Some(ChangeRequestTimelineCommit {
                    oid: text(&commit["oid"]).unwrap_or_default(),
                    message: text(&commit["messageHeadline"]).unwrap_or_default(),
                    author: text(&commit["author"]["name"]),
                }),
                ..Default::default()
            });
        }
        "PullRequestReview" => {
            return Some(ChangeRequestTimelineItem {
                kind: "review".into(),
                actor: actor(&node["author"]),
                created_at: text(&node["submittedAt"])
                    .or_else(|| text(&node["createdAt"]))
                    .unwrap_or_default(),
                url: text(&node["url"]),
                state: text(&node["state"]),
                ..Default::default()
            });
        }
        "IssueComment" => {
            return Some(ChangeRequestTimelineItem {
                kind: "comment".into(),
                actor: actor(&node["author"]),
                created_at: text(&node["createdAt"]).unwrap_or_default(),
                url: text(&node["url"]),
                ..Default::default()
            });
        }
        "HeadRefForcePushedEvent" => "forcePush",
        "ReviewRequestedEvent" => "reviewRequested",
        "ReviewRequestRemovedEvent" => "reviewRequestRemoved",
        "LabeledEvent" => "labeled",
        "UnlabeledEvent" => "unlabeled",
        "ReadyForReviewEvent" => "readyForReview",
        "ConvertToDraftEvent" => "convertedToDraft",
        "BaseRefChangedEvent" => "baseRefChanged",
        "AutoMergeEnabledEvent" => "autoMergeEnabled",
        "AutoMergeDisabledEvent" => "autoMergeDisabled",
        "MergedEvent" => "merged",
        "ClosedEvent" => "closed",
        "ReopenedEvent" => "reopened",
        "CrossReferencedEvent" => "crossReferenced",
        "ConnectedEvent" => "connected",
        "DisconnectedEvent" => "disconnected",
        _ => return None,
    };
    let source = node.get("source").or_else(|| node.get("subject"));
    Some(ChangeRequestTimelineItem {
        kind: kind.into(),
        actor: actor(&node["actor"]),
        created_at: text(&node["createdAt"]).unwrap_or_default(),
        name: text(&node["label"]["name"]).or_else(|| reviewer_name(&node["requestedReviewer"])),
        color: text(&node["label"]["color"]),
        from: text(&node["previousRefName"]).or_else(|| text(&node["beforeCommit"]["oid"])),
        to: text(&node["currentRefName"])
            .or_else(|| text(&node["afterCommit"]["oid"]))
            .or_else(|| text(&node["commit"]["oid"])),
        subject: source
            .filter(|source| source["number"].is_u64())
            .map(|source| subject(source, source["__typename"] == "PullRequest")),
        ..Default::default()
    })
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn actor(value: &Value) -> Option<ChangeRequestActor> {
    Some(ChangeRequestActor {
        login: text(&value["login"])?,
        avatar_url: text(&value["avatarUrl"]),
    })
}

fn reviewer_name(reviewer: &Value) -> Option<String> {
    text(&reviewer["login"]).or_else(|| text(&reviewer["name"]))
}

fn subject(node: &Value, is_pull_request: bool) -> ChangeRequestTimelineSubject {
    ChangeRequestTimelineSubject {
        number: node["number"].as_u64().unwrap_or_default(),
        title: text(&node["title"]).unwrap_or_default(),
        url: text(&node["url"]).unwrap_or_default(),
        state: text(&node["state"]).unwrap_or_default(),
        is_pull_request,
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
