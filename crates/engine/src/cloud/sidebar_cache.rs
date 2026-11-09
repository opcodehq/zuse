//! Read-only catalog snapshots, never compute authority or agent credentials.
use serde_json::{Value, json};
const PROJECTS: &str = zuse_protocol::methods::CLOUD_PROJECTS;
const LIST: &str = zuse_protocol::methods::CLOUD_WORKSPACES;
pub(super) fn supports(method: &str) -> bool {
    matches!(method, PROJECTS | LIST)
}
fn namespace(origin: &str, account: &str, organization: Option<&str>, method: &str) -> String {
    zuse_cloud_projection::digest(
        &serde_json::to_vec(&(origin, account, organization, method)).unwrap(),
    )
}
fn snapshot(method: &str, value: &Value) -> Value {
    let (rows, fields): (&Value, &[&str]) = if method == PROJECTS {
        (
            &value["data"]["projects"],
            &[
                "projectId",
                "displayName",
                "repositoryIdentity",
                "repositoryUrl",
                "name",
                "title",
                "defaultBranch",
                "baseRef",
                "providerId",
                "environmentRevision",
                "environmentConfigured",
                "canCreate",
            ],
        )
    } else {
        (
            &value["workspaces"],
            &[
                "workspaceId",
                "projectId",
                "title",
                "branch",
                "state",
                "desiredState",
                "runtimeState",
                "initialSessionId",
                "chatId",
                "providerId",
                "updatedAt",
                "createdAt",
                "rpcProtocol",
                "statusCode",
            ],
        )
    };
    let mut saved = vec![];
    let mut partial = false;
    for row in rows.as_array().into_iter().flatten() {
        let mut clean = serde_json::Map::new();
        for key in fields {
            if let Some(value) = row.get(*key) {
                if value.is_string() || value.is_boolean() || value.is_number() || value.is_null() {
                    clean.insert((*key).to_owned(), value.clone());
                }
            }
        }
        saved.push(Value::Object(clean));
        if serde_json::to_vec(&saved).map_or(true, |v| v.len() > 60000) {
            saved.pop();
            partial = true;
            break;
        }
    }
    if method == PROJECTS {
        json!({"data":{"projects":saved},"cachePartial":partial})
    } else {
        json!({"workspaces":saved,"cachePartial":partial})
    }
}
pub(super) fn read(origin: &str, account: &str, organization: Option<&str>, method: &str) -> Value {
    let mut value = zuse_cloud_projection::protected_metadata::read(&namespace(
        origin,
        account,
        organization,
        method,
    ))
    .unwrap_or_else(|| json!({"cacheMiss":true}));
    value["accountId"] = json!(account);
    value["organizationId"] = json!(organization);
    value["apiOrigin"] = json!(origin);
    value["provisional"] = json!(true);
    value
}
pub(super) fn store(
    origin: &str,
    account: &str,
    organization: Option<&str>,
    method: &str,
    value: &Value,
) {
    if supports(method) {
        zuse_cloud_projection::protected_metadata::write(
            &namespace(origin, account, organization, method),
            &snapshot(method, value),
        );
    }
}
pub(super) fn clear_all() {
    zuse_cloud_projection::protected_metadata::clear(None);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_namespace_isolates_origin_actor_org_and_kind() {
        let base = namespace("https://a", "actor", None, PROJECTS);
        for other in [
            namespace("https://b", "actor", None, PROJECTS),
            namespace("https://a", "other", None, PROJECTS),
            namespace("https://a", "actor", Some("org"), PROJECTS),
            namespace("https://a", "actor", None, LIST),
        ] {
            assert_ne!(base, other);
        }
    }
    #[test]
    fn stored_catalog_has_no_credentials_authority_or_transcript_payloads() {
        let v = snapshot(
            PROJECTS,
            &json!({"data":{"projects":[{"projectId":"p","repositoryIdentity":"github.com/o/p","token":"secret","environment":{"secret":"hidden"}}]},"agentConnections":[{"credential":"secret"}]}),
        );
        assert_eq!(v["data"]["projects"][0]["projectId"], "p");
        let text = v.to_string();
        assert!(
            !text.contains("secret")
                && !text.contains("credential")
                && !text.contains("agentConnections")
        );
        let v = snapshot(
            LIST,
            &json!({"workspaces":[{"workspaceId":"w","state":"paused","bootstrap":{"secret":"hidden"},"messages":["private text"]}]}),
        );
        assert_eq!(v["workspaces"][0]["state"], "paused");
        assert!(!v.to_string().contains("private text"));
    }
}
