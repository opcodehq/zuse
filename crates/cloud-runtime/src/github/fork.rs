//! Pull requests from forks. A maintainer may push to a fork's pull request
//! branch when its author allows edits from maintainers; GitHub authorizes that
//! push against the base repository, which is the one repository our token is
//! scoped to. The broker hands the same token to a fork remote only when an open
//! pull request on the base repository comes from that fork and allows edits.
//! GitHub remains the real boundary: it accepts such pushes only on the pull
//! request's head branch.
use super::*;
use std::collections::HashMap;

/// A positive answer stays valid briefly so one `git push` (several helper
/// calls) does one lookup. A refusal expires sooner so ticking "Allow edits
/// by maintainers" takes effect on the next try.
const ALLOWED_FOR_MS: i64 = 60_000;
const REFUSED_FOR_MS: i64 = 10_000;
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;

/// Line the broker adds to a refusal. The helper prints it to stderr (git
/// shows helper stderr) and never passes it to git.
pub(super) const MESSAGE_KEY: &str = "zuse-message=";

pub(super) struct ForkPolicy {
    client: reqwest::Client,
    api: reqwest::Url,
    decisions: Mutex<HashMap<String, (bool, i64)>>,
}

#[derive(serde::Deserialize)]
struct Pull {
    #[serde(default)]
    maintainer_can_modify: bool,
    head: Head,
}
#[derive(serde::Deserialize)]
struct Head {
    repo: Option<HeadRepository>,
}
#[derive(serde::Deserialize)]
struct HeadRepository {
    full_name: String,
    #[serde(default)]
    fork: bool,
}

impl ForkPolicy {
    pub(super) fn new() -> Result<Self> {
        Self::with_api(reqwest::Url::parse("https://api.github.com/")?)
    }
    pub(super) fn with_api(api: reqwest::Url) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            api,
            decisions: Mutex::new(HashMap::new()),
        })
    }

    /// Whether `fork` is the head repository of an open pull request on `base`
    /// that allows edits from maintainers.
    pub(super) async fn allows(&self, base: &str, fork: &str, token: &str) -> Result<bool> {
        let key = fork.to_ascii_lowercase();
        let mut decisions = self.decisions.lock().await;
        let now = now_ms();
        decisions.retain(|_, (allowed, at)| {
            now - *at
                < if *allowed {
                    ALLOWED_FOR_MS
                } else {
                    REFUSED_FOR_MS
                }
        });
        if let Some((allowed, _)) = decisions.get(&key) {
            return Ok(*allowed);
        }
        let allowed = self.lookup(base, fork, token).await?;
        decisions.insert(key, (allowed, now));
        Ok(allowed)
    }

    async fn lookup(&self, base: &str, fork: &str, token: &str) -> Result<bool> {
        let (owner, name) = base
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("github_repository_invalid"))?;
        for page in 1..=MAX_PAGES {
            let mut url = self.api.clone();
            url.path_segments_mut()
                .map_err(|_| anyhow::anyhow!("github_api_invalid"))?
                .pop_if_empty()
                .extend(["repos", owner, name, "pulls"]);
            url.query_pairs_mut()
                .append_pair("state", "open")
                .append_pair("per_page", &PAGE_SIZE.to_string())
                .append_pair("page", &page.to_string());
            // The request carries the bearer token: never log it or its headers.
            let mut response = self
                .client
                .get(url)
                .bearer_auth(token)
                .header("accept", "application/vnd.github+json")
                .header("x-github-api-version", "2022-11-28")
                .header("user-agent", "zuse-cloud-runtime")
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("github_pull_requests_unavailable"))?;
            ensure!(
                response.status().is_success(),
                "github_pull_requests_unavailable"
            );
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("github_pull_requests_unavailable"))?
            {
                ensure!(
                    body.len() + chunk.len() <= MAX_PAGE_BYTES,
                    "github_pull_requests_unavailable"
                );
                body.extend_from_slice(&chunk);
            }
            let pulls: Vec<Pull> = serde_json::from_slice(&body)
                .map_err(|_| anyhow::anyhow!("github_pull_requests_unavailable"))?;
            if pulls.iter().any(|pull| editable_from(pull, fork)) {
                return Ok(true);
            }
            if pulls.len() < PAGE_SIZE {
                break;
            }
        }
        Ok(false)
    }
}

fn editable_from(pull: &Pull, fork: &str) -> bool {
    pull.maintainer_can_modify
        && pull
            .head
            .repo
            .as_ref()
            .is_some_and(|repo| repo.fork && repo.full_name.eq_ignore_ascii_case(fork))
}

/// What git receives when the broker will not hand over a token for `fork`:
/// `quit=1` stops git from prompting or trying other helpers.
pub(super) fn refusal(base: &str, fork: &str, reason: Refusal) -> Zeroizing<Vec<u8>> {
    let why = match reason {
        Refusal::NotEditable => {
            format!("no open pull request on {base} from {fork} allows edits by maintainers")
        }
        Refusal::Unavailable => {
            format!("could not check the open pull requests on {base}")
        }
    };
    Zeroizing::new(
        format!(
            "quit=1\n{MESSAGE_KEY}Zuse can't push to {fork}: {why}. Push the fix to a branch on {base} instead (git push https://github.com/{base}.git HEAD:<new-branch>), then open a pull request or tell the user where the fix is.\n\n"
        )
        .into_bytes(),
    )
}
#[derive(Clone, Copy)]
pub(super) enum Refusal {
    NotEditable,
    Unavailable,
}

/// Split a broker answer into what git reads and a message for the user.
pub(super) fn split_message(response: &[u8]) -> (Zeroizing<Vec<u8>>, Option<String>) {
    let Ok(text) = std::str::from_utf8(response) else {
        return (Zeroizing::new(response.to_vec()), None);
    };
    let mut message = None;
    let mut forwarded = Zeroizing::new(String::with_capacity(text.len()));
    for line in text.split_inclusive('\n') {
        match line.strip_prefix(MESSAGE_KEY) {
            Some(value) => {
                message = Some(
                    value
                        .trim_end()
                        .chars()
                        .filter(|c| !c.is_control())
                        .collect(),
                )
            }
            None => forwarded.push_str(line),
        }
    }
    (Zeroizing::new(forwarded.as_bytes().to_vec()), message)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use axum::{Json, Router, extract::Query, http::HeaderMap, routing::get};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(in crate::github) fn pull(head: &str, fork: bool, editable: bool) -> Value {
        json!({"number":1,"maintainer_can_modify":editable,"head":{"ref":"feature","repo":{"full_name":head,"fork":fork}}})
    }

    pub(in crate::github) async fn serve(pages: Vec<Vec<Value>>) -> (ForkPolicy, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let pages = Arc::new(pages);
        let app = Router::new().route(
            "/repos/team/project/pulls",
            get(
                move |headers: HeaderMap,
                      Query(query): Query<std::collections::HashMap<String, String>>| {
                    let pages = pages.clone();
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(headers["authorization"], "Bearer scoped-token");
                        assert_eq!(query["state"], "open");
                        let page: usize = query["page"].parse().unwrap();
                        Json(Value::Array(pages.get(page - 1).cloned().unwrap_or_default()))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let policy = ForkPolicy::with_api(format!("http://{address}/").parse().unwrap()).unwrap();
        (policy, calls)
    }

    #[tokio::test]
    async fn a_fork_is_writable_only_through_an_open_pull_request_that_allows_edits() {
        let (policy, _) = serve(vec![vec![
            pull("contributor/project", true, true),
            pull("locked/project", true, false),
            pull("team/project", false, true),
        ]])
        .await;
        let allows = |fork: &'static str| policy.allows("team/project", fork, "scoped-token");
        assert!(allows("contributor/project").await.unwrap());
        assert!(allows("Contributor/Project").await.unwrap());
        assert!(!allows("locked/project").await.unwrap());
        assert!(!allows("stranger/project").await.unwrap());
    }

    #[tokio::test]
    async fn later_pages_are_searched_and_answers_are_cached() {
        let first: Vec<_> = (0..PAGE_SIZE)
            .map(|i| pull(&format!("someone{i}/project"), true, false))
            .collect();
        let (policy, calls) = serve(vec![first, vec![pull("late/project", true, true)]]).await;
        assert!(
            policy
                .allows("team/project", "late/project", "scoped-token")
                .await
                .unwrap()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            policy
                .allows("team/project", "late/project", "scoped-token")
                .await
                .unwrap()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_lookup_is_an_error_not_a_grant() {
        let policy = ForkPolicy::with_api("http://127.0.0.1:9/".parse().unwrap()).unwrap();
        assert!(
            policy
                .allows("team/project", "contributor/project", "scoped-token")
                .await
                .is_err()
        );
    }

    #[test]
    fn a_refusal_stops_git_and_tells_the_user_how_to_continue() {
        let response = refusal("team/project", "contributor/project", Refusal::NotEditable);
        let (git, message) = split_message(&response);
        assert_eq!(git.as_slice(), b"quit=1\n\n");
        let message = message.unwrap();
        assert!(message.contains("contributor/project"));
        assert!(message.contains("allows edits by maintainers"));
        assert!(message.contains("git push https://github.com/team/project.git HEAD:"));
        let (git, message) = split_message(b"username=actor\npassword=scoped-token\n\n");
        assert_eq!(git.as_slice(), b"username=actor\npassword=scoped-token\n\n");
        assert!(message.is_none());
    }
}
