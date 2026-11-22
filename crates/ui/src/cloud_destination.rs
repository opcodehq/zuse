//! Where a new chat runs, and when the composer offers Cloud. A subscribed
//! account with a connected repository always sees Cloud: on the chat's
//! repository when Cloud knows it, otherwise as a choice of the connected
//! repositories. Cloud that can't start yet stays listed with the reason
//! instead of vanishing. A new chat in a project backed by a Cloud
//! repository starts on Cloud.
use serde_json::Value;

/// Where "New chat" in a sidebar project starts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NewChat {
    /// On Cloud with this repository, keeping the project's local checkout
    /// (if any) as the Local and Worktree alternative.
    Cloud {
        project: Value,
        local_space: Option<String>,
    },
    /// On the local checkout.
    Local(Option<String>),
}

/// A project backed by a Cloud repository starts its new chats on Cloud for
/// a subscriber; otherwise on its local checkout. A project that only exists
/// in Cloud always goes to Cloud (unsubscribed accounts land on Billing).
pub(crate) fn new_chat(
    cloud_project: Option<&Value>,
    local_space: Option<&str>,
    subscribed: bool,
) -> NewChat {
    match (cloud_project, local_space) {
        (Some(project), local) if subscribed || local.is_none() => NewChat::Cloud {
            project: project.clone(),
            local_space: local.map(str::to_owned),
        },
        (_, local) => NewChat::Local(local.map(str::to_owned)),
    }
}

/// The connected repository a local project's remote points at.
pub(crate) fn matching_project<'a>(local_repository: &str, rows: &'a [Value]) -> Option<&'a Value> {
    let local = normalize(local_repository);
    rows.iter().find(|row| {
        row["repositoryIdentity"]
            .as_str()
            .is_some_and(|identity| normalize(identity) == local)
    })
}

/// How the picker offers Cloud for the chat being written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CloudOffer {
    /// Not offered: local-only build, signed out, unsubscribed, or no
    /// repository connected to Cloud.
    Hidden,
    /// Cloud on this connected repository.
    Repository(Value),
    /// The chat's project lives on GitHub (`owner/repo`) but Cloud doesn't
    /// have it yet: picking Cloud connects it first.
    Connect(String),
    /// The chat has no repository Cloud knows: picking Cloud asks which
    /// connected repository to use.
    ChooseRepository,
    /// Listed but not startable yet, with why.
    Unavailable(String),
}

/// What the offer is decided from.
pub(crate) struct Facts<'a> {
    /// Subscribed, or the subscription read is still in flight.
    pub offers_cloud: bool,
    /// The repository already chosen for this chat.
    pub selected: Option<&'a Value>,
    /// The chat's local project remote, as `host/owner/repo`.
    pub local_repository: Option<&'a str>,
    /// The account's connected repositories.
    pub rows: &'a [Value],
    /// Whether new computers can start, else why not.
    pub ready: Result<(), &'a str>,
}

pub(crate) fn offer(facts: &Facts) -> CloudOffer {
    if !facts.offers_cloud {
        return CloudOffer::Hidden;
    }
    let destination = destination(facts.selected, facts.local_repository, facts.rows);
    let connect = facts.local_repository.and_then(github_repository);
    if destination.is_none() && connect.is_none() && !facts.rows.iter().any(launchable) {
        return CloudOffer::Hidden;
    }
    if let Err(reason) = facts.ready {
        return CloudOffer::Unavailable(reason.to_owned());
    }
    match (destination, connect) {
        (Some(row), _) => CloudOffer::Repository(row),
        (None, Some(repository)) => CloudOffer::Connect(repository),
        (None, None) => CloudOffer::ChooseRepository,
    }
}

/// A remote as `owner/repo`, when it lives on GitHub.
pub(crate) fn github_repository(remote: &str) -> Option<String> {
    normalize(remote)
        .strip_prefix("github.com/")
        .filter(|rest| rest.contains('/'))
        .map(str::to_owned)
}

/// The chat's Cloud repository: the one already chosen, else the connected
/// repository its local project's remote points at.
pub(crate) fn destination(
    selected: Option<&Value>,
    local_repository: Option<&str>,
    rows: &[Value],
) -> Option<Value> {
    if let Some(selected) = selected {
        return Some(selected.clone());
    }
    matching_project(local_repository?, rows).cloned()
}

/// A connected repository a new chat can start on.
pub(crate) fn launchable(row: &Value) -> bool {
    row["canCreate"] != false
}

/// One spelling for a repository: lowercase `host/owner/repo`, whether it
/// came as a URL, an SSH remote or with a `.git` suffix.
pub(crate) fn normalize(identity: &str) -> String {
    let mut value = identity.trim().to_ascii_lowercase();
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = value.strip_prefix(scheme) {
            value = rest.to_owned();
        }
    }
    if let Some(rest) = value.strip_prefix("git@") {
        value = rest.replacen(':', "/", 1);
    }
    if let Some((_, rest)) = value.split_once('@') {
        value = rest.to_owned();
    }
    let value = value.trim_end_matches('/');
    value.strip_suffix(".git").unwrap_or(value).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows() -> Vec<Value> {
        vec![
            json!({"projectId":"zuse","repositoryIdentity":"github.com/opcodehq/zuse"}),
            json!({"projectId":"zkit","repositoryIdentity":"github.com/opcodehq/zkit"}),
        ]
    }
    fn facts<'a>(rows: &'a [Value], local: Option<&'a str>) -> Facts<'a> {
        Facts {
            offers_cloud: true,
            selected: None,
            local_repository: local,
            rows,
            ready: Ok(()),
        }
    }

    #[test]
    fn a_subscribed_account_gets_cloud_on_the_chats_repository() {
        let rows = rows();
        assert_eq!(
            offer(&facts(&rows, Some("github.com/opcodehq/zuse"))),
            CloudOffer::Repository(rows[0].clone())
        );
        // The remote's own spelling still matches.
        for local in ["GitHub.com/OpcodeHQ/Zuse", "github.com/opcodehq/zuse.git"] {
            assert_eq!(
                offer(&facts(&rows, Some(local))),
                CloudOffer::Repository(rows[0].clone()),
                "{local}"
            );
        }
        // A repository already chosen wins over the local project.
        let chosen = rows[1].clone();
        assert_eq!(
            offer(&Facts {
                selected: Some(&chosen),
                ..facts(&rows, Some("github.com/opcodehq/zuse"))
            }),
            CloudOffer::Repository(chosen.clone())
        );
    }

    #[test]
    fn a_chat_without_a_cloud_repository_asks_which_one() {
        let rows = rows();
        // No project at all, or a project off GitHub Cloud can't connect.
        assert_eq!(offer(&facts(&rows, None)), CloudOffer::ChooseRepository);
        assert_eq!(
            offer(&facts(&rows, Some("gitlab.com/someone/else"))),
            CloudOffer::ChooseRepository
        );
    }

    #[test]
    fn a_github_project_cloud_lacks_is_connected_on_pick() {
        // Even before any repository is connected.
        for rows in [rows(), Vec::new()] {
            assert_eq!(
                offer(&facts(&rows, Some("git@github.com:someone/else.git"))),
                CloudOffer::Connect("someone/else".into())
            );
        }
    }

    #[test]
    fn cloud_that_cannot_start_says_why_instead_of_hiding() {
        let rows = rows();
        assert_eq!(
            offer(&Facts {
                ready: Err("Connect an agent account first."),
                ..facts(&rows, Some("github.com/opcodehq/zuse"))
            }),
            CloudOffer::Unavailable("Connect an agent account first.".into())
        );
    }

    #[test]
    fn hidden_only_without_a_subscription_or_any_connected_repository() {
        let rows = rows();
        assert_eq!(
            offer(&Facts {
                offers_cloud: false,
                ..facts(&rows, Some("github.com/opcodehq/zuse"))
            }),
            CloudOffer::Hidden
        );
        assert_eq!(offer(&facts(&[], None)), CloudOffer::Hidden);
        // Only repositories that can't start a chat: nothing to choose.
        let blocked = vec![
            json!({"projectId":"zuse","repositoryIdentity":"github.com/opcodehq/zuse","canCreate":false}),
        ];
        assert_eq!(offer(&facts(&blocked, None)), CloudOffer::Hidden);
    }

    #[test]
    fn a_new_chat_in_a_cloud_project_starts_on_cloud() {
        let rows = rows();
        // A sidebar project backed by both a Cloud repository and a local
        // checkout: Cloud for a subscriber, with the checkout kept.
        assert_eq!(
            new_chat(Some(&rows[0]), Some("space-zuse"), true),
            NewChat::Cloud {
                project: rows[0].clone(),
                local_space: Some("space-zuse".into())
            }
        );
        // Without a subscription the checkout stays the default.
        assert_eq!(
            new_chat(Some(&rows[0]), Some("space-zuse"), false),
            NewChat::Local(Some("space-zuse".into()))
        );
        // Cloud-only and local-only projects.
        assert!(matches!(
            new_chat(Some(&rows[1]), None, true),
            NewChat::Cloud {
                local_space: None,
                ..
            }
        ));
        assert_eq!(
            new_chat(None, Some("space"), true),
            NewChat::Local(Some("space".into()))
        );
    }

    #[test]
    fn a_local_project_maps_to_its_cloud_repository_by_remote() {
        let rows = rows();
        assert_eq!(
            matching_project("github.com/OpcodeHQ/zuse", &rows),
            Some(&rows[0])
        );
        assert_eq!(matching_project("github.com/swarajbachu/zuse", &rows), None);
    }

    #[test]
    fn repository_spellings_normalize() {
        for spelling in [
            "github.com/opcodehq/zuse",
            "https://github.com/opcodehq/zuse.git",
            "git@github.com:opcodehq/zuse.git",
            "ssh://git@github.com/opcodehq/zuse",
            "github.com/opcodehq/zuse/",
        ] {
            assert_eq!(
                normalize(spelling),
                "github.com/opcodehq/zuse",
                "{spelling}"
            );
        }
    }
}
