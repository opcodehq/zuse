//! Display grouping only. A cloud task keeps its own account and computer identity.
use crate::Space;

/// A repository as `host/owner/repo`, lowercase, from any spelling of it: a
/// remote URL (https or ssh, with or without `.git`) or an identity. `None`
/// for anything that is not a repository location (a local-only identity
/// such as `commit:<sha>` or `local:<hash>`).
pub fn repository_key(spelling: &str) -> Option<String> {
    // Case never distinguishes repositories: `HTTPS://…/Repo.GIT` is the same.
    let lower = spelling.trim().to_ascii_lowercase();
    let mut text = lower.as_str();
    let mut scheme = false;
    for prefix in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest;
            scheme = true;
            break;
        }
    }
    // `git@github.com:owner/repo` and `user@host/owner/repo`.
    if let Some((user, rest)) = text.split_once('@')
        && !user.contains('/')
    {
        text = rest;
    }
    // The authority ends at the first `/` of a URL, or at the `:` of the
    // scp form (`host:owner/repo`); a URL's `:port` is dropped.
    let (authority, path) = match (text.find(':'), text.find('/')) {
        (Some(colon), slash) if !scheme && slash.is_none_or(|slash| colon < slash) => {
            (&text[..colon], &text[colon + 1..])
        }
        _ => text.split_once('/')?,
    };
    let host = authority.split(':').next()?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 || !host.contains('.') {
        return None;
    }
    Some(format!("{host}/{}", parts.join("/")))
}

/// `owner/repo` for a repository spelling, in its own case.
pub fn repository_owner_and_name(spelling: &str) -> Option<String> {
    repository_key(spelling)?;
    let text = spelling.trim().trim_end_matches('/');
    let text = match text.len().checked_sub(4) {
        Some(at) if text.is_char_boundary(at) && text[at..].eq_ignore_ascii_case(".git") => {
            &text[..at]
        }
        _ => text,
    };
    let mut parts = text.rsplit(['/', ':']).filter(|part| !part.is_empty());
    let repo = parts.next()?;
    let owner = parts.next()?;
    Some(format!("{owner}/{repo}"))
}

/// One project as a list shows it: its short name and, when known, the
/// repository it is a checkout of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLabelInput {
    pub name: String,
    pub repository: Option<String>,
}

/// Labels for a list of projects: each keeps its short name unless another
/// project of a different repository has the same one; then every
/// repository-backed project of that name is labelled `owner/repo`
/// ("opcodehq/zuse" and "swarajbachu/zuse" instead of two "zuse"). Pure.
pub fn project_labels(projects: &[ProjectLabelInput]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let mut repositories: HashMap<String, HashSet<Option<String>>> = HashMap::new();
    for project in projects {
        repositories
            .entry(project.name.to_lowercase())
            .or_default()
            .insert(project.repository.as_deref().and_then(repository_key));
    }
    projects
        .iter()
        .map(|project| {
            let collides = repositories
                .get(&project.name.to_lowercase())
                .is_some_and(|set| set.len() > 1);
            collides
                .then(|| {
                    project
                        .repository
                        .as_deref()
                        .and_then(repository_owner_and_name)
                })
                .flatten()
                .unwrap_or_else(|| project.name.clone())
        })
        .collect()
}

/// The short name of a cloud project: its repository name, not the
/// `owner/repo` path some display names carry.
pub fn cloud_project_short_name(project: &serde_json::Value) -> String {
    project["displayName"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .or(project["repositoryIdentity"].as_str())
        .and_then(|name| name.trim_end_matches('/').rsplit('/').next())
        .filter(|name| !name.is_empty())
        .unwrap_or("Cloud project")
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudProjectGroup {
    pub key: String,
    pub name: String,
    pub project_id: String,
    pub local_space: Option<String>,
}

pub fn cloud_project_group(
    project: &serde_json::Value,
    spaces: &[Space],
) -> Option<CloudProjectGroup> {
    let project_id = project["projectId"].as_str().filter(|id| !id.is_empty())?;
    let identity = project["repositoryIdentity"]
        .as_str()
        .filter(|id| !id.is_empty());
    // Linked by repository (the checkout's origin remote against the cloud
    // repository), never by a name two repositories can share.
    let wanted = identity.and_then(repository_key);
    let matches = |space: &&Space| {
        wanted.as_ref().is_some_and(|wanted| {
            [
                space.remote_repository.as_deref(),
                space.repository_id.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|spelling| repository_key(spelling).as_ref() == Some(wanted))
        })
    };
    // A monorepo subfolder carries the repository's remote too; the
    // repository's root checkout is the project the cloud repository is.
    let matched = spaces
        .iter()
        .filter(matches)
        .min_by_key(|space| (in_repository_subfolder(space), space.created_at, &space.id))
        .or_else(|| unremoted_namesake(project, spaces));
    let representative = matched.map(|space| crate::view::representative_space(spaces, space));
    Some(CloudProjectGroup {
        key: matched
            .map(crate::view::project_key)
            .unwrap_or_else(|| format!("cloud-project:{project_id}")),
        name: representative
            .map(|space| space.display_name().to_owned())
            .unwrap_or_else(|| cloud_project_short_name(project)),
        project_id: project_id.to_owned(),
        local_space: representative.map(|space| space.id.clone()),
    })
}

/// A space below its repository's top level: its identity carries the
/// in-repository path (`commit:<sha>:apps/web`, `host/owner/repo:apps/web`).
fn in_repository_subfolder(space: &Space) -> bool {
    space.repository_id.as_deref().is_some_and(|id| {
        id.strip_prefix("commit:")
            .or_else(|| id.strip_prefix("local:"))
            .unwrap_or(id)
            .contains(':')
    })
}

/// The one local repository project named like the cloud project when no
/// checkout of it says which repository it is (no remote stamped yet, an
/// older engine on another device, no `origin`). A project whose remote
/// names any repository never joins by name, and neither does a name two
/// local projects share.
fn unremoted_namesake<'a>(project: &serde_json::Value, spaces: &'a [Space]) -> Option<&'a Space> {
    let name = cloud_project_short_name(project);
    let mut found: Option<&Space> = None;
    for space in spaces.iter().filter(|space| {
        !crate::projectless::is_projectless_cwd(Some(&space.path))
            && crate::view::representative_space(spaces, space)
                .display_name()
                .eq_ignore_ascii_case(&name)
    }) {
        let remote = [
            space.remote_repository.as_deref(),
            space.repository_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|spelling| repository_key(spelling).is_some());
        if remote || !space.git_detected || in_repository_subfolder(space) {
            return None;
        }
        match found {
            Some(other) if crate::view::project_key(other) != crate::view::project_key(space) => {
                return None;
            }
            Some(_) => {}
            None => found = Some(space),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repository_spellings_reduce_to_one_key() {
        for spelling in [
            "https://github.com/opcodehq/zuse.git",
            "git@github.com:opcodehq/zuse.git",
            "ssh://git@github.com/OpcodeHQ/zuse",
            "github.com/opcodehq/zuse",
            "https://github.com/opcodehq/zuse/",
            "HTTPS://GitHub.com/OpcodeHQ/Zuse.GIT",
            "ssh://git@github.com:22/opcodehq/zuse.git",
            "https://github.com:443/opcodehq/zuse",
        ] {
            assert_eq!(
                repository_key(spelling).as_deref(),
                Some("github.com/opcodehq/zuse"),
                "{spelling}"
            );
        }
        assert_eq!(repository_key("commit:abc"), None);
        assert_eq!(repository_key("zuse"), None);
        assert_eq!(
            repository_owner_and_name("git@github.com:swarajbachu/zuse.git").as_deref(),
            Some("swarajbachu/zuse")
        );
        assert_eq!(
            repository_owner_and_name("HTTPS://github.com/OpcodeHQ/Zuse.GIT").as_deref(),
            Some("OpcodeHQ/Zuse"),
            "labels keep the original spelling"
        );
    }

    #[test]
    fn only_colliding_project_names_show_their_owner() {
        let input = |name: &str, repository: Option<&str>| ProjectLabelInput {
            name: name.into(),
            repository: repository.map(str::to_owned),
        };
        let labels = project_labels(&[
            input("zuse", Some("https://github.com/opcodehq/zuse.git")),
            input("zuse", Some("github.com/swarajbachu/zuse")),
            input("zuse-review", Some("github.com/opcodehq/zuse-review")),
            input("legion", None),
            // The same repository listed twice is not a collision.
            input("zkit", Some("github.com/opcodehq/zkit")),
            input("zkit", Some("git@github.com:opcodehq/zkit.git")),
        ]);
        assert_eq!(
            labels,
            [
                "opcodehq/zuse",
                "swarajbachu/zuse",
                "zuse-review",
                "legion",
                "zkit",
                "zkit"
            ]
        );
    }

    #[test]
    fn cloud_tasks_join_the_local_repository_group_without_changing_checkout_identity() {
        let spaces = vec![
            serde_json::from_value::<Space>(json!({
                "id":"local-project","deviceId":"mac","path":"/work/my-app",
                "repositoryId":"commit:root","remoteRepository":"github.com/Owner/App",
                "createdAt":"2026-01-01T00:00:00Z"
            }))
            .unwrap(),
        ];
        let project = json!({"projectId":"cloud-project","repositoryIdentity":"github.com/owner/app","displayName":"App"});
        let group = cloud_project_group(&project, &spaces).unwrap();
        assert_eq!(group.key, crate::view::project_key(&spaces[0]));
        assert_eq!(group.local_space.as_deref(), Some("local-project"));
        assert_eq!(group.name, "my-app");
        assert_eq!(group.project_id, "cloud-project");
    }

    #[test]
    fn cloud_only_projects_are_visible_and_matching_names_do_not_merge_unrelated_repositories() {
        let cloud = json!({"projectId":"cloud-id","repositoryIdentity":"github.com/owner/app","displayName":"app"});
        let no_local = cloud_project_group(&cloud, &[]).unwrap();
        assert_eq!(no_local.key, "cloud-project:cloud-id");
        let unrelated =
            vec![serde_json::from_value::<Space>(json!({
            "id":"local","deviceId":"mac","path":"/work/app","repositoryId":"commit:another",
            "remoteRepository":"github.com/another/app","createdAt":"2026-01-01T00:00:00Z"
        })).unwrap()];
        assert_eq!(cloud_project_group(&cloud, &unrelated).unwrap(), no_local);
    }

    fn space(id: &str, path: &str, repository: Option<&str>, remote: Option<&str>) -> Space {
        serde_json::from_value(json!({
            "id": id, "deviceId": "mac", "path": path, "gitDetected": true,
            "repositoryId": repository, "remoteRepository": remote,
            "createdAt": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn the_repository_root_wins_over_a_monorepo_subfolder() {
        let spaces = vec![
            space(
                "web",
                "/work/app/apps/web",
                Some("commit:root:apps/web"),
                Some("github.com/owner/app"),
            ),
            space(
                "root",
                "/work/app",
                Some("commit:root"),
                Some("github.com/owner/app"),
            ),
        ];
        let project = json!({"projectId":"p","repositoryIdentity":"github.com/owner/app","displayName":"app"});
        let group = cloud_project_group(&project, &spaces).unwrap();
        assert_eq!(group.local_space.as_deref(), Some("root"));
        assert_eq!(group.key, "repo:commit:root");
    }

    #[test]
    fn a_checkout_without_a_known_remote_joins_its_namesake_only_when_unambiguous() {
        let project = json!({"projectId":"p","repositoryIdentity":"github.com/opcodehq/delulu","displayName":"opcodehq/delulu"});
        // No remote stamped: the one local "delulu" is this repository.
        let unstamped = vec![
            space("a", "/work/delulu", Some("commit:root"), None),
            space("b", "/other/delulu", Some("commit:root"), None),
        ];
        let group = cloud_project_group(&project, &unstamped).unwrap();
        assert_eq!(group.key, "repo:commit:root");
        assert_eq!(group.name, "delulu");
        // A remote naming another repository never joins by name.
        let fork = vec![space(
            "a",
            "/work/delulu",
            Some("commit:root"),
            Some("github.com/someone/delulu"),
        )];
        assert_eq!(
            cloud_project_group(&project, &fork).unwrap().key,
            "cloud-project:p"
        );
        // Two unrelated local projects of that name: no guess.
        let two = vec![
            space("a", "/work/delulu", Some("commit:one"), None),
            space("b", "/old/delulu", Some("commit:two"), None),
        ];
        assert_eq!(
            cloud_project_group(&project, &two).unwrap().key,
            "cloud-project:p"
        );
        // Home and scratch folders are never projects.
        assert_eq!(
            cloud_project_group(&project, &[]).unwrap().key,
            "cloud-project:p"
        );
    }
}
