//! Existing organization memberships are checked at connection, command dispatch,
//! and during the gateway lease. This module never creates memberships or seats.
use super::*;
use crate::{
    access::{Access, Permission, Scope},
    auth::Identity,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
struct Observation {
    membership: crate::auth::Membership,
    until: Instant,
    github_revision: u64,
    enrollment: Option<Value>,
}
pub struct ScopeAuthority {
    identity: Arc<dyn Identity>,
    pub(super) github: Option<Arc<crate::github::native::NativeGithub>>,
    organizations: Vec<String>,
    personal_workspaces: tokio::sync::Mutex<std::collections::HashSet<String>>,
    members:
        tokio::sync::Mutex<HashMap<(String, String), Arc<tokio::sync::Mutex<Option<Observation>>>>>,
}
impl RuntimeControl {
    pub(crate) fn scope_authority_configured(&self) -> bool {
        self.scope_authority.get().is_some()
    }
    /// Used after a confirmed membership/policy change. Do not leave an older
    /// in-flight refresh installed after invalidation.
    pub async fn invalidate_organization_access(&self, organization: &str, actor: &str) {
        if let Some(authority) = self.scope_authority.get() {
            let entry = authority
                .members
                .lock()
                .await
                .get(&(actor.to_owned(), organization.to_owned()))
                .cloned();
            if let Some(entry) = entry {
                *entry.lock().await = None;
            }
        }
    }
    pub fn configure_scope_authority(
        &self,
        identity: Arc<dyn Identity>,
        github: Option<Arc<crate::github::native::NativeGithub>>,
        organizations: Vec<String>,
    ) -> Result<(), ApiError> {
        self.scope_authority
            .set(Arc::new(ScopeAuthority {
                identity,
                github,
                organizations,
                members: Default::default(),
                personal_workspaces: Default::default(),
            }))
            .map_err(|_| ApiError::conflict("workspace_scope_authority_already_configured"))
    }
    /// Run before opening a workspace transaction: authorization may perform HTTP
    /// and must never consume the pool behind a held workspace/advisory lock.
    pub(super) async fn scope_access(
        &self,
        id: &str,
        actor: Option<&str>,
        edit: bool,
    ) -> Result<Option<(Access, Value)>, ApiError> {
        if let Some(authority) = self.scope_authority.get() {
            if authority.personal_workspaces.lock().await.contains(id) {
                return Ok(None);
            }
        }
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        let owner = required(&row, "account_id")?;
        let Some(_org) = owner.strip_prefix("organization:") else {
            if let Some(authority) = self.scope_authority.get() {
                let mut cache = authority.personal_workspaces.lock().await;
                if cache.len() >= 100_000 {
                    cache.clear();
                }
                cache.insert(id.to_owned());
            }
            return Ok(None);
        };
        let actor = actor
            .or_else(|| row["request_config"]["rustActorId"].as_str())
            .ok_or_else(|| ApiError::conflict("github_task_actor_migration_required"))?;
        let access = self.scope_actor(owner, actor).await?;
        let permission = access.workspace_permission(owner, &row["request_config"])?;
        if edit && permission != Permission::Edit {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        Ok(Some((
            access,
            row["request_config"]["sharingPolicy"].clone(),
        )))
    }
    /// Admission/upload has no task row yet; it still checks existing membership
    /// and the same GitHub-managed enrollment/revision policy as execution.
    pub(crate) async fn scope_admission(&self, access: &Access) -> Result<(), ApiError> {
        if !super::creation::creation_actor_allowed(access) {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if access.membership.is_none() {
            return Ok(());
        }
        let fresh = self
            .scope_actor(&access.owner_id, &access.actor.account_id)
            .await?;
        if fresh.membership.as_ref().map(|m| m.id.as_str())
            != access.membership.as_ref().map(|m| m.id.as_str())
        {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(())
    }
    pub(crate) async fn scope_actor(&self, owner: &str, actor: &str) -> Result<Access, ApiError> {
        let org = owner
            .strip_prefix("organization:")
            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?;
        let authority = self
            .scope_authority
            .get()
            .ok_or_else(|| ApiError::conflict("workspace_scope_authority_required"))?;
        if !authority.organizations.iter().any(|id| id == org) {
            return Err(ApiError::forbidden("organization_workspaces_disabled"));
        }
        let enrollment:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('enrollment',e.data,'installationId',e.installation_id,'policy',(SELECT p.data FROM api_github_join_policies p WHERE p.organization_id=e.organization_id AND p.installation_id=e.installation_id)) FROM api_github_enrollments e WHERE e.organization_id=$1 AND e.account_id=$2").bind(org).bind(actor).fetch_optional(&self.pool).await?;
        if let Some(record) = &enrollment {
            let e = &record["enrollment"];
            let p = &record["policy"];
            if e["blocked"] != false
                || e["githubOrgId"] != p["githubOrgId"]
                || record["installationId"] != e["installationId"]
                || p["installationId"] != record["installationId"]
                || e["revision"].as_str().is_none()
                || p["revision"].as_str().is_none()
            {
                return Err(ApiError::forbidden("organization_access_denied"));
            }
        }
        let key = (actor.to_owned(), org.to_owned());
        let entry = {
            let mut map = authority.members.lock().await;
            if map.len() > 10_000 {
                map.retain(|_, value| Arc::strong_count(value) > 1);
            }
            map.entry(key).or_default().clone()
        };
        let mut cached = entry.lock().await;
        let membership = if let Some(current) = cached.as_ref().filter(|o| {
            o.until > Instant::now()
                && o.enrollment == enrollment
                && o.github_revision
                    == authority
                        .github
                        .as_ref()
                        .map_or(0, |g| g.membership_revision())
        }) {
            current.membership.clone()
        } else {
            let membership = authority.identity.membership(actor, org).await?;
            if membership.status != "active"
                || membership.user_id != actor
                || membership.organization_id != org
                || !matches!(membership.role.slug.as_str(), "admin" | "member")
            {
                return Err(ApiError::forbidden("organization_access_denied"));
            }
            if let Some(record) = &enrollment {
                let e = &record["enrollment"];
                if e["memberId"].as_str().is_some_and(|id| id != membership.id) {
                    return Err(ApiError::forbidden("organization_access_denied"));
                }
                authority
                    .github
                    .as_ref()
                    .ok_or_else(|| ApiError::conflict("github_enrollment_migration_required"))?
                    .verify_organization_member(
                        owner,
                        actor,
                        record["installationId"]
                            .as_i64()
                            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?,
                        e["githubUserId"]
                            .as_i64()
                            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?,
                        e["githubOrgId"]
                            .as_i64()
                            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?,
                    )
                    .await?;
                let current:Option<Value>=sqlx::query_scalar("SELECT jsonb_build_object('enrollment',e.data,'installationId',e.installation_id,'policy',(SELECT p.data FROM api_github_join_policies p WHERE p.organization_id=e.organization_id AND p.installation_id=e.installation_id)) FROM api_github_enrollments e WHERE e.organization_id=$1 AND e.account_id=$2").bind(org).bind(actor).fetch_optional(&self.pool).await?;
                if current.as_ref() != Some(record) {
                    return Err(ApiError::forbidden("organization_access_denied"));
                }
            }
            *cached = Some(Observation {
                membership: membership.clone(),
                enrollment: enrollment.clone(),
                until: Instant::now() + Duration::from_secs(10),
                github_revision: authority
                    .github
                    .as_ref()
                    .map_or(0, |g| g.membership_revision()),
            });
            membership
        };
        drop(cached);
        Ok(Access {
            actor: crate::auth::Principal {
                account_id: actor.to_owned(),
            },
            scope: Scope::Organization {
                organization_id: org.to_owned(),
            },
            owner_id: owner.to_owned(),
            membership: Some(membership),
        })
    }
    pub(super) async fn runtime_scope_access(
        &self,
        id: &str,
        credential: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        if !row["account_id"]
            .as_str()
            .is_some_and(|owner| owner.starts_with("organization:"))
        {
            return Ok(());
        }
        super::mailbox::runtime_auth(&row, credential, now)?;
        self.scope_access(id, None, true).await?;
        Ok(())
    }
    pub(super) async fn renewal_scope_access(
        &self,
        id: &str,
        credential: &str,
        request: &RenewRequest,
        now: i64,
    ) -> Result<(), ApiError> {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        if !row["account_id"]
            .as_str()
            .is_some_and(|owner| owner.starts_with("organization:"))
        {
            return Ok(());
        }
        let config = &row["request_config"];
        let current = hash(credential);
        let replay = config["runtimeCredentialRenewal"]["requestId"] == request.request_id
            && config["runtimeCredentialRenewal"]["previousCredentialHash"]
                .as_str()
                .is_some_and(|h| equal(h, &current));
        if blocked(&row)
            || row["desired_state"] != "ready"
            || !enrollment_matches(&row)
            || (!replay
                && row["runtime_credential_hash"]
                    .as_str()
                    .is_none_or(|h| !equal(h, &current)))
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if let Some(public) = config["runtimeSigningPublicJwk"].as_str() {
            if config["runtimeSigningKeyThumbprint"] != signing_thumbprint(public)? {
                return Err(ApiError::unauthorized(
                    "runtime_signing_key_binding_mismatch",
                ));
            }
            let generation = config["runtimeGeneration"].as_i64().unwrap_or(1);
            let epoch = config["gatewayEpoch"].as_i64().unwrap_or(generation);
            verify_renewal_proof(
                &request.proof,
                public,
                &self.signer.issuer(),
                id,
                &request.request_id,
                (generation, epoch),
                now,
            )?;
        } else if config["runtimeCredentialExpiresAtMs"].as_i64().unwrap_or(0) <= now {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        self.scope_access(id, None, true).await?;
        Ok(())
    }
    pub async fn revalidate_gateway_peer(
        &self,
        grant: &crate::gateway::Grant,
    ) -> Result<(), ApiError> {
        let checked = self
            .scope_access(
                &grant.workspace,
                (!grant.runtime).then_some(grant.actor.as_str()),
                grant.runtime || grant.permission == "edit",
            )
            .await?;
        if let Some((access, _)) = checked {
            if !grant.runtime && access.actor.account_id != grant.actor {
                return Err(ApiError::forbidden("workspace_access_denied"));
            }
        }
        Ok(())
    }
}

/// Worker-side admission accepts only the certified native task contract created
/// by an authorized request. Current membership is rechecked by API enrollment,
/// model/Git grants, gateway and command dispatch before execution is exposed.
pub(crate) fn native_organization_task(row: &Value) -> bool {
    let Some(owner) = row["account_id"].as_str() else {
        return false;
    };
    if !owner.starts_with("organization:") {
        return true;
    }
    let config = &row["request_config"];
    let Some(actor) = config["rustActorId"]
        .as_str()
        .filter(|id| !id.is_empty() && !id.starts_with("organization:"))
    else {
        return false;
    };
    config["rustOrganizationExecutionVersion"] == 1
        && config["providerAuthMode"] == "broker-v1"
        && config["managedReleaseId"].is_string()
        && config["agentConnectionId"].is_string()
        && config["agentConnectionOwnerId"]
            .as_str()
            .is_some_and(|id| id == owner || id == actor)
        && config["sharingPolicy"]["creatorSubject"] == actor
        && config["sharingPolicy"]["creatorMembershipId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::{Membership, MembershipRole, Principal},
        crypto::MasterKey,
        tickets::TicketSigner,
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct IdentityFixture {
        removed: AtomicBool,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Identity for IdentityFixture {
        async fn verify(&self, _: &str) -> Result<Principal, ApiError> {
            unreachable!()
        }
        async fn membership(&self, actor: &str, org: &str) -> Result<Membership, ApiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.removed.load(Ordering::SeqCst) {
                return Err(ApiError::forbidden("organization_access_denied"));
            }
            Ok(Membership {
                id: format!("membership-{actor}"),
                user_id: actor.into(),
                organization_id: org.into(),
                status: "active".into(),
                role: MembershipRole {
                    slug: if actor == "admin" { "admin" } else { "member" }.into(),
                },
            })
        }
    }
    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn organization_scope_rechecks_membership_policy_and_departure() {
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("scope_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={search}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,request_config jsonb);CREATE TABLE api_github_enrollments(organization_id text,account_id text,installation_id bigint,data jsonb);CREATE TABLE api_github_join_policies(organization_id text,installation_id bigint,data jsonb);").execute(&pool).await.unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[87; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([87;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([88; 32])).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        );
        let identity = Arc::new(IdentityFixture {
            removed: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
        });
        control
            .configure_scope_authority(identity.clone(), None, vec!["team".into()])
            .unwrap();
        let policy = json!({"audience":"private","permission":"view","creatorSubject":"creator","creatorMembershipId":"membership-creator","grants":[],"revision":1});
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','organization:team',$1)")
            .bind(json!({"rustActorId":"creator","sharingPolicy":policy}))
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            control
                .scope_access("task", None, true)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            control
                .scope_access("task", Some("collaborator"), false)
                .await
                .is_err()
        );
        assert!(
            control
                .scope_access("task", Some("admin"), true)
                .await
                .is_ok()
        );
        let calls = identity.calls.load(Ordering::SeqCst);
        control.scope_access("task", None, true).await.unwrap();
        assert_eq!(identity.calls.load(Ordering::SeqCst), calls);
        // Sharing changes are checked from current durable policy despite cached membership.
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{sharingPolicy,creatorMembershipId}', '\"removed-membership\"') WHERE workspace_id='task'").execute(&pool).await.unwrap();
        assert!(control.scope_access("task", None, true).await.is_err());
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{sharingPolicy,creatorMembershipId}', '\"membership-creator\"') WHERE workspace_id='task'").execute(&pool).await.unwrap();
        identity.removed.store(true, Ordering::SeqCst);
        let entry = control
            .scope_authority
            .get()
            .unwrap()
            .members
            .lock()
            .await
            .get(&("creator".into(), "team".into()))
            .unwrap()
            .clone();
        entry.lock().await.as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert!(control.scope_access("task", None, true).await.is_err());
        let peer = crate::gateway::Grant {
            workspace: "task".into(),
            generation: 1,
            epoch: 1,
            runtime: false,
            protocol: zuse_protocol::cloud_gateway::RUST_PROTOCOL.into(),
            actor: "creator".into(),
            permission: "edit".into(),
            expires_at: chrono::Utc::now().timestamp_millis() + 60000,
        };
        assert!(control.revalidate_gateway_peer(&peer).await.is_err());
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
