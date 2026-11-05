//! Display grouping only. A cloud task keeps its own account and computer identity.
use super::*;
use crate::Space;

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
    let matched = identity.and_then(|identity| {
        spaces.iter().find(|space| {
            space
                .remote_repository
                .as_deref()
                .is_some_and(|remote| remote.eq_ignore_ascii_case(identity))
                || space
                    .repository_id
                    .as_deref()
                    .is_some_and(|repository| repository.eq_ignore_ascii_case(identity))
        })
    });
    let representative = matched.map(|space| crate::view::representative_space(spaces, space));
    Some(CloudProjectGroup {
        key: matched
            .map(crate::view::project_key)
            .unwrap_or_else(|| format!("cloud-project:{project_id}")),
        name: representative
            .map(|space| space.display_name().to_owned())
            .unwrap_or_else(|| {
                project["displayName"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .or(identity.and_then(|id| id.rsplit('/').next()))
                    .unwrap_or("Cloud project")
                    .to_owned()
            }),
        project_id: project_id.to_owned(),
        local_space: representative.map(|space| space.id.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
