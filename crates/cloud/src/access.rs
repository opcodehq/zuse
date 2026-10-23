use crate::{
    auth::{Identity, Membership, Principal},
    error::ApiError,
};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Scope {
    Personal,
    Organization {
        #[serde(rename = "organizationId")]
        organization_id: String,
    },
}

impl Scope {
    pub fn parse(value: Option<&str>) -> Result<Self, ApiError> {
        match value {
            None | Some("personal") => Ok(Self::Personal),
            Some(value) => value
                .strip_prefix("organization:")
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                })
                .map(|id| Self::Organization {
                    organization_id: id.to_owned(),
                })
                .ok_or_else(|| ApiError::bad_request("invalid_workspace_scope")),
        }
    }
}

#[derive(Clone, Copy)]
pub enum AccessKind {
    Content,
    Administration,
    Billing,
}

pub struct Access {
    pub actor: Principal,
    pub scope: Scope,
    pub owner_id: String,
    pub membership: Option<Membership>,
}

pub async fn authorize(
    identity: &dyn Identity,
    headers: &HeaderMap,
    kind: AccessKind,
    organizations_enabled: bool,
    organization_ids: &[String],
) -> Result<Access, ApiError> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let (scheme, token) = raw
        .split_once(' ')
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.is_empty())
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let _ = scheme;
    let actor = identity.verify(token).await?;
    if headers.get_all("x-zuse-workspace").iter().count() > 1 {
        return Err(ApiError::bad_request("invalid_workspace_scope"));
    }
    let scope_header = headers
        .get("x-zuse-workspace")
        .map(|v| {
            v.to_str()
                .map_err(|_| ApiError::bad_request("invalid_workspace_scope"))
        })
        .transpose()?;
    let scope = Scope::parse(scope_header)?;
    let (owner_id, membership) = match &scope {
        Scope::Personal => (actor.account_id.clone(), None),
        Scope::Organization { organization_id } => {
            if !organizations_enabled || !organization_ids.contains(organization_id) {
                return Err(ApiError::forbidden("organization_workspaces_disabled"));
            }
            let member = identity
                .membership(&actor.account_id, organization_id)
                .await?;
            let allowed = member.role.slug == "admin"
                || (matches!(kind, AccessKind::Content) && member.role.slug == "member")
                || (matches!(kind, AccessKind::Billing) && member.role.slug == "billing");
            if !allowed {
                return Err(ApiError::forbidden("workspace_access_denied"));
            }
            (format!("organization:{organization_id}"), Some(member))
        }
    };
    Ok(Access {
        actor,
        scope,
        owner_id,
        membership,
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    View,
    Edit,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Audience {
    Private,
    Organization,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grant {
    membership_id: String,
    permission: Permission,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    audience: Audience,
    permission: Permission,
    creator_subject: String,
    creator_membership_id: String,
    grants: Vec<Grant>,
    revision: Option<u64>,
}

impl Access {
    pub fn workspace_permission(
        &self,
        owner_id: &str,
        config: &Value,
    ) -> Result<Permission, ApiError> {
        if owner_id != self.owner_id {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let Some(member) = &self.membership else {
            return Ok(Permission::Edit);
        };
        if member.role.slug == "admin" {
            return Ok(Permission::Edit);
        }
        if member.role.slug != "member" {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let policy: Policy = serde_json::from_value(config["sharingPolicy"].clone())
            .map_err(|_| ApiError::forbidden("workspace_sharing_policy_invalid"))?;
        let _ = policy.revision;
        if policy.creator_subject.is_empty()
            || policy.creator_membership_id.is_empty()
            || policy.grants.iter().any(|g| g.membership_id.is_empty())
        {
            return Err(ApiError::forbidden("workspace_sharing_policy_invalid"));
        }
        if policy.creator_subject == self.actor.account_id
            && policy.creator_membership_id == member.id
        {
            return Ok(Permission::Edit);
        }
        let mut permissions = policy
            .grants
            .iter()
            .filter(|g| g.membership_id == member.id)
            .map(|g| g.permission)
            .collect::<Vec<_>>();
        if matches!(policy.audience, Audience::Organization) {
            permissions.push(policy.permission);
        }
        if permissions.contains(&Permission::Edit) {
            Ok(Permission::Edit)
        } else if permissions.contains(&Permission::View) {
            Ok(Permission::View)
        } else {
            Err(ApiError::forbidden("workspace_access_denied"))
        }
    }
}
