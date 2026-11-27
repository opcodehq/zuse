//! Pull request webhook payloads → what to record. Pure; no database.
use serde_json::Value;
use zuse_protocol::pull_request_links::PullRequestEventKind;

/// Longest comment, review or check summary kept for the chat.
pub(crate) const BODY_LIMIT: usize = 4000;

/// Which owned pull requests an event is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Number(i64),
    /// Check results name their pull requests only for same-repository
    /// branches; otherwise they are matched by head commit.
    Checks { numbers: Vec<i64>, head_sha: String },
}

/// New values for the owner row's copy of the pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Update {
    pub title: Option<String>,
    pub state: Option<&'static str>,
    pub draft: Option<bool>,
    pub head_ref: Option<String>,
    pub head_sha: Option<String>,
}

/// Something to tell the owning chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub kind: PullRequestEventKind,
    /// Unique per event across deliveries (GitHub's comment, review or check id).
    pub event_id: String,
    pub url: String,
    pub author: Option<String>,
    pub author_id: Option<i64>,
    pub bot: bool,
    pub body: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Classified {
    pub installation_id: i64,
    pub repository_id: i64,
    pub target: Target,
    pub update: Update,
    pub draft: Option<Draft>,
}

/// GitHub event names this handles; every other event is left alone.
pub(crate) fn handles(event: &str) -> bool {
    matches!(
        event,
        "pull_request"
            | "issue_comment"
            | "pull_request_review"
            | "pull_request_review_comment"
            | "check_run"
            | "check_suite"
    )
}

pub(crate) fn classify(event: &str, value: &Value) -> Option<Classified> {
    let installation_id = positive(&value["installation"]["id"])?;
    let repository_id = positive(&value["repository"]["id"])?;
    let action = value["action"].as_str().unwrap_or("");
    let sender = &value["sender"];
    let (target, update, draft) = match event {
        "pull_request" => {
            let pr = &value["pull_request"];
            let number = positive(&pr["number"])?;
            let update = pull_request_update(pr);
            let kind = match action {
                "closed" if pr["merged"] == true => Some(PullRequestEventKind::Merged),
                "closed" => Some(PullRequestEventKind::Closed),
                "reopened" => Some(PullRequestEventKind::Reopened),
                "opened" | "edited" | "synchronize" | "ready_for_review"
                | "converted_to_draft" => None,
                _ => return None,
            };
            let draft = kind.map(|kind| Draft {
                kind,
                event_id: format!("pr:{number}:{action}:{}", pr["updated_at"].as_str().unwrap_or("")),
                url: url(&pr["html_url"]),
                author: login(sender),
                author_id: positive(&sender["id"]),
                bot: bot(sender),
                body: String::new(),
                location: None,
            });
            (Target::Number(number), update, draft)
        }
        "issue_comment" => {
            let issue = &value["issue"];
            // Issue comments on plain issues carry no `pull_request` object.
            if action != "created" || !issue["pull_request"].is_object() {
                return None;
            }
            let comment = &value["comment"];
            let user = &comment["user"];
            (
                Target::Number(positive(&issue["number"])?),
                Update::default(),
                Some(Draft {
                    kind: PullRequestEventKind::Comment,
                    event_id: format!("comment:{}", positive(&comment["id"])?),
                    url: url(&comment["html_url"]),
                    author: login(user),
                    author_id: positive(&user["id"]),
                    bot: bot(user),
                    body: bounded(comment["body"].as_str().unwrap_or("")),
                    location: None,
                }),
            )
        }
        "pull_request_review" => {
            let review = &value["review"];
            let state = review["state"].as_str().unwrap_or("").to_ascii_lowercase();
            let body = bounded(review["body"].as_str().unwrap_or(""));
            // A plain "commented" review with no body only wraps inline
            // comments, which arrive as their own events.
            if action != "submitted" || (state == "commented" && body.trim().is_empty()) {
                return None;
            }
            let verdict = match state.as_str() {
                "approved" => "Approved",
                "changes_requested" => "Requested changes",
                _ => "Commented",
            };
            let user = &review["user"];
            (
                Target::Number(positive(&value["pull_request"]["number"])?),
                Update::default(),
                Some(Draft {
                    kind: PullRequestEventKind::Review,
                    event_id: format!("review:{}", positive(&review["id"])?),
                    url: url(&review["html_url"]),
                    author: login(user),
                    author_id: positive(&user["id"]),
                    bot: bot(user),
                    body: if body.trim().is_empty() {
                        verdict.into()
                    } else {
                        format!("{verdict}: {body}")
                    },
                    location: None,
                }),
            )
        }
        "pull_request_review_comment" => {
            if action != "created" {
                return None;
            }
            let comment = &value["comment"];
            let user = &comment["user"];
            let path = comment["path"].as_str().filter(|p| !p.is_empty());
            let line = comment["line"]
                .as_u64()
                .or_else(|| comment["original_line"].as_u64());
            (
                Target::Number(positive(&value["pull_request"]["number"])?),
                Update::default(),
                Some(Draft {
                    kind: PullRequestEventKind::ReviewComment,
                    event_id: format!("review-comment:{}", positive(&comment["id"])?),
                    url: url(&comment["html_url"]),
                    author: login(user),
                    author_id: positive(&user["id"]),
                    bot: bot(user),
                    body: bounded(comment["body"].as_str().unwrap_or("")),
                    location: path.map(|path| match line {
                        Some(line) => format!("{}:{line}", clip(path, 300)),
                        None => clip(path, 300),
                    }),
                }),
            )
        }
        "check_run" => {
            let run = &value["check_run"];
            if action != "completed" || !failed(run["conclusion"].as_str()) {
                return None;
            }
            let name = clip(run["name"].as_str().unwrap_or("A check"), 200);
            let title = run["output"]["title"].as_str().unwrap_or("").trim();
            let summary = run["output"]["summary"].as_str().unwrap_or("").trim();
            let mut body = format!(
                "{name} {}",
                match run["conclusion"].as_str() {
                    Some("timed_out") => "timed out",
                    Some("action_required") => "needs action",
                    _ => "failed",
                }
            );
            for part in [title, summary] {
                if !part.is_empty() {
                    body.push_str("\n\n");
                    body.push_str(part);
                }
            }
            let app = &run["app"];
            (
                checks_target(&run["pull_requests"], run["head_sha"].as_str(), repository_id)?,
                Update::default(),
                Some(Draft {
                    kind: PullRequestEventKind::ChecksFailed,
                    event_id: format!("check-run:{}", positive(&run["id"])?),
                    url: url(run.get("details_url").filter(|v| v.is_string()).unwrap_or(&run["html_url"])),
                    author: app["name"].as_str().map(|n| clip(n, 100)),
                    author_id: None,
                    bot: false,
                    body: bounded(&body),
                    location: None,
                }),
            )
        }
        "check_suite" => {
            let suite = &value["check_suite"];
            if action != "completed" || suite["conclusion"].as_str() != Some("success") {
                return None;
            }
            let head = suite["head_sha"].as_str()?;
            (
                checks_target(&suite["pull_requests"], Some(head), repository_id)?,
                Update::default(),
                Some(Draft {
                    kind: PullRequestEventKind::ChecksPassed,
                    event_id: format!("check-suite:{}", positive(&suite["id"])?),
                    url: String::new(),
                    author: suite["app"]["name"].as_str().map(|n| clip(n, 100)),
                    author_id: None,
                    bot: false,
                    body: "Checks passed".into(),
                    location: None,
                }),
            )
        }
        _ => return None,
    };
    Some(Classified {
        installation_id,
        repository_id,
        target,
        update,
        draft,
    })
}

fn pull_request_update(pr: &Value) -> Update {
    Update {
        title: pr["title"].as_str().map(|t| clip(t, 500)),
        state: Some(state(pr)),
        draft: pr["draft"].as_bool(),
        head_ref: pr["head"]["ref"]
            .as_str()
            .filter(|r| r.len() <= 255)
            .map(str::to_owned),
        head_sha: pr["head"]["sha"].as_str().filter(|s| sha_ok(s)).map(str::to_owned),
    }
}

/// `open`, `closed` or `merged` from a REST pull request object.
pub(crate) fn state(pr: &Value) -> &'static str {
    if pr["merged"] == true || pr["merged_at"].is_string() {
        "merged"
    } else if pr["state"] == "closed" {
        "closed"
    } else {
        "open"
    }
}

fn checks_target(pull_requests: &Value, head: Option<&str>, repository_id: i64) -> Option<Target> {
    let head_sha = head.filter(|s| sha_ok(s))?.to_owned();
    // A check on a fork's commit can list pull requests of other repositories.
    let numbers = pull_requests
        .as_array()
        .into_iter()
        .flatten()
        .filter(|pr| positive(&pr["base"]["repo"]["id"]).is_none_or(|id| id == repository_id))
        .filter_map(|pr| positive(&pr["number"]))
        .take(20)
        .collect();
    Some(Target::Checks { numbers, head_sha })
}

fn failed(conclusion: Option<&str>) -> bool {
    matches!(
        conclusion,
        Some("failure" | "timed_out" | "action_required" | "startup_failure")
    )
}

fn positive(value: &Value) -> Option<i64> {
    value.as_i64().filter(|n| *n > 0)
}

fn login(user: &Value) -> Option<String> {
    user["login"]
        .as_str()
        .filter(|l| !l.is_empty() && l.len() <= 100)
        .map(str::to_owned)
}

fn bot(user: &Value) -> bool {
    user["type"] == "Bot" || user["login"].as_str().is_some_and(|l| l.ends_with("[bot]"))
}

fn url(value: &Value) -> String {
    value
        .as_str()
        .filter(|u| u.starts_with("https://") && u.len() <= 512)
        .unwrap_or("")
        .to_owned()
}

fn sha_ok(sha: &str) -> bool {
    (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

fn bounded(text: &str) -> String {
    clip(text.trim(), BODY_LIMIT)
}

/// Whether someone wrote to the agent by name: their own comments still wake it.
pub(crate) fn mentions_zuse(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.match_indices("@zuse").any(|(at, _)| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + 5..].chars().next();
        before.is_none_or(|c| !c.is_ascii_alphanumeric())
            && after.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({"installation":{"id":9},"repository":{"id":44,"full_name":"acme/app"},"sender":{"login":"ada","id":1,"type":"User"}})
    }
    fn with(mut value: Value, extra: Value) -> Value {
        for (k, v) in extra.as_object().unwrap() {
            value[k] = v.clone();
        }
        value
    }

    #[test]
    fn issue_comments_on_pull_requests_become_comments() {
        let value = with(
            base(),
            json!({"action":"created","issue":{"number":7,"pull_request":{}},"comment":{"id":55,"body":"  Rename this  ","html_url":"https://github.com/acme/app/pull/7#issuecomment-55","user":{"login":"bob","id":2,"type":"User"}}}),
        );
        let c = classify("issue_comment", &value).unwrap();
        assert_eq!(c.installation_id, 9);
        assert_eq!(c.repository_id, 44);
        assert_eq!(c.target, Target::Number(7));
        let d = c.draft.unwrap();
        assert_eq!(d.kind, PullRequestEventKind::Comment);
        assert_eq!(d.event_id, "comment:55");
        assert_eq!(d.body, "Rename this");
        assert_eq!(d.author.as_deref(), Some("bob"));
        assert_eq!(d.author_id, Some(2));
        assert!(!d.bot);
    }

    #[test]
    fn plain_issue_comments_and_edits_are_ignored() {
        let plain = with(
            base(),
            json!({"action":"created","issue":{"number":7},"comment":{"id":55,"body":"x","user":{"login":"bob"}}}),
        );
        assert!(classify("issue_comment", &plain).is_none());
        let edited = with(
            base(),
            json!({"action":"edited","issue":{"number":7,"pull_request":{}},"comment":{"id":55,"body":"x","user":{"login":"bob"}}}),
        );
        assert!(classify("issue_comment", &edited).is_none());
    }

    #[test]
    fn bots_are_recognised() {
        let value = with(
            base(),
            json!({"action":"created","issue":{"number":7,"pull_request":{}},"comment":{"id":56,"body":"Coverage 80%","user":{"login":"codecov[bot]","id":3,"type":"Bot"}}}),
        );
        assert!(classify("issue_comment", &value).unwrap().draft.unwrap().bot);
    }

    #[test]
    fn reviews_carry_their_verdict_and_empty_comment_reviews_are_skipped() {
        let review = |state: &str, body: &str| {
            with(
                base(),
                json!({"action":"submitted","pull_request":{"number":3},"review":{"id":8,"state":state,"body":body,"html_url":"https://github.com/acme/app/pull/3#pullrequestreview-8","user":{"login":"bob","id":2}}}),
            )
        };
        let changes = classify("pull_request_review", &review("CHANGES_REQUESTED", "Split it"))
            .unwrap()
            .draft
            .unwrap();
        assert_eq!(changes.kind, PullRequestEventKind::Review);
        assert_eq!(changes.body, "Requested changes: Split it");
        let approved = classify("pull_request_review", &review("approved", ""))
            .unwrap()
            .draft
            .unwrap();
        assert_eq!(approved.body, "Approved");
        assert!(classify("pull_request_review", &review("commented", "")).is_none());
    }

    #[test]
    fn inline_review_comments_keep_their_location() {
        let value = with(
            base(),
            json!({"action":"created","pull_request":{"number":3},"comment":{"id":91,"body":"Off by one","path":"src/lib.rs","line":42,"html_url":"https://github.com/acme/app/pull/3#discussion_r91","user":{"login":"bob","id":2}}}),
        );
        let d = classify("pull_request_review_comment", &value)
            .unwrap()
            .draft
            .unwrap();
        assert_eq!(d.kind, PullRequestEventKind::ReviewComment);
        assert_eq!(d.location.as_deref(), Some("src/lib.rs:42"));
        assert_eq!(d.event_id, "review-comment:91");
    }

    #[test]
    fn pull_request_events_update_the_copy_and_report_merges() {
        let pr = |action: &str, merged: bool| {
            with(
                base(),
                json!({"action":action,"pull_request":{"number":3,"title":"Fix","state":if action=="closed"{"closed"}else{"open"},"merged":merged,"draft":false,"updated_at":"2026-11-27T10:00:00Z","html_url":"https://github.com/acme/app/pull/3","head":{"ref":"fix","sha":"abcdef1234567"}}}),
            )
        };
        let pushed = classify("pull_request", &pr("synchronize", false)).unwrap();
        assert!(pushed.draft.is_none());
        assert_eq!(pushed.update.head_sha.as_deref(), Some("abcdef1234567"));
        assert_eq!(pushed.update.state, Some("open"));
        let merged = classify("pull_request", &pr("closed", true)).unwrap();
        assert_eq!(merged.update.state, Some("merged"));
        assert_eq!(merged.draft.unwrap().kind, PullRequestEventKind::Merged);
        let closed = classify("pull_request", &pr("closed", false)).unwrap();
        assert_eq!(closed.draft.unwrap().kind, PullRequestEventKind::Closed);
        assert!(classify("pull_request", &pr("labeled", false)).is_none());
    }

    #[test]
    fn failed_check_runs_target_their_pull_requests_and_head() {
        let value = with(
            base(),
            json!({"action":"completed","check_run":{"id":5,"name":"test","conclusion":"failure","head_sha":"abcdef1234567","details_url":"https://github.com/acme/app/actions/runs/1","output":{"title":"2 failed","summary":"lib::a"},"app":{"name":"GitHub Actions"},"pull_requests":[{"number":3,"base":{"repo":{"id":44}}},{"number":9,"base":{"repo":{"id":99}}}]}}),
        );
        let c = classify("check_run", &value).unwrap();
        assert_eq!(
            c.target,
            Target::Checks {
                numbers: vec![3],
                head_sha: "abcdef1234567".into()
            }
        );
        let d = c.draft.unwrap();
        assert_eq!(d.kind, PullRequestEventKind::ChecksFailed);
        assert_eq!(d.body, "test failed\n\n2 failed\n\nlib::a");
        assert_eq!(d.url, "https://github.com/acme/app/actions/runs/1");
        let passed = with(
            base(),
            json!({"action":"completed","check_run":{"id":6,"name":"test","conclusion":"success","head_sha":"abcdef1234567"}}),
        );
        assert!(classify("check_run", &passed).is_none());
    }

    #[test]
    fn successful_suites_are_passes_and_others_are_skipped() {
        let suite = |conclusion: &str| {
            with(
                base(),
                json!({"action":"completed","check_suite":{"id":12,"conclusion":conclusion,"head_sha":"abcdef1234567","pull_requests":[]}}),
            )
        };
        let c = classify("check_suite", &suite("success")).unwrap();
        assert_eq!(c.draft.unwrap().kind, PullRequestEventKind::ChecksPassed);
        assert!(classify("check_suite", &suite("failure")).is_none());
    }

    #[test]
    fn events_without_installation_or_repository_are_dropped() {
        let value = json!({"action":"created","issue":{"number":7,"pull_request":{}},"comment":{"id":55,"body":"x","user":{"login":"bob"}},"repository":{"id":44}});
        assert!(classify("issue_comment", &value).is_none());
    }

    #[test]
    fn zuse_mentions_need_a_word_boundary() {
        assert!(mentions_zuse("@zuse please fix"));
        assert!(mentions_zuse("hey @Zuse, rename"));
        assert!(!mentions_zuse("@zuser said"));
        assert!(!mentions_zuse("mail me@zuse.sh"));
        assert!(!mentions_zuse("nothing here"));
    }

    #[test]
    fn long_bodies_are_clipped() {
        let long = "x".repeat(BODY_LIMIT + 10);
        assert_eq!(bounded(&long).chars().count(), BODY_LIMIT + 1);
    }
}
