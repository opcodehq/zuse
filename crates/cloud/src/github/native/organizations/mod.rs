//! GitHub-driven enrollment preserves the pinned TypeScript seat and membership
//! rules. OAuth identity proof is a separate prerequisite; never accepts a
//! caller-supplied GitHub identity from an HTTP body.
use super::*;
use async_trait::async_trait;
use std::collections::BTreeSet;
mod approval;
mod continuation;
mod directory;
mod jobs;
mod reconcile;
mod roster;
mod setup;
mod startup;
pub use setup::{SetupCallback, legacy_setup_forward, setup_notice};
#[cfg(test)]
mod tests;
pub use directory::WorkosDirectory;
pub use roster::GithubRoster;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub organization_id: String,
    pub installation_id: i64,
    pub github_org_id: i64,
    pub login: String,
    pub enabled: bool,
    pub revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrollment {
    pub organization_id: String,
    pub account_id: String,
    pub github_user_id: i64,
    pub installation_id: i64,
    pub github_org_id: i64,
    pub blocked: bool,
    pub member_id: Option<String>,
    pub reserved_until: i64,
    pub revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedIdentity {
    pub account_id: String,
    pub github_user_id: i64,
    pub login: String,
    pub organization_ids: Vec<i64>,
    pub verification_id: String,
}
#[derive(Clone, Deserialize)]
pub struct Member {
    pub id: String,
    pub user_id: String,
    pub organization_id: String,
    pub status: String,
    #[serde(default)]
    pub directory_managed: bool,
    pub role: crate::auth::MembershipRole,
}
#[derive(Deserialize)]
pub struct Invitation {
    pub organization_id: Option<String>,
    pub state: String,
    pub expires_at: String,
}
#[async_trait]
pub trait Directory: Send + Sync {
    async fn members(&self, org: &str) -> Result<Vec<Member>, ApiError>;
    async fn invitations(&self, org: &str) -> Result<Vec<Invitation>, ApiError>;
    async fn admit(
        &self,
        org: &str,
        actor: &str,
        previous: Option<&str>,
    ) -> Result<Member, ApiError>;
    async fn deactivate(&self, id: &str) -> Result<(), ApiError>;
}
#[async_trait]
pub trait Roster: Send + Sync {
    /// Must return a complete, freshly authorized organization roster. A failed
    /// page or disconnected/suspended installation is an error, never an empty set.
    async fn members(&self, policy: &Policy) -> Result<BTreeSet<i64>, ApiError>;
}
pub struct Joining {
    pool: PgPool,
    directory: Arc<dyn Directory>,
    roster: Arc<dyn Roster>,
    organizations: BTreeSet<String>,
    dispatch: tokio::sync::Notify,
}
impl Joining {
    pub fn new(
        pool: PgPool,
        directory: Arc<dyn Directory>,
        roster: Arc<dyn Roster>,
        organizations: Vec<String>,
    ) -> Self {
        Self {
            pool,
            directory,
            roster,
            organizations: organizations.into_iter().collect(),
            dispatch: tokio::sync::Notify::new(),
        }
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        let exists: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_github_join_jobs') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !exists {
            return Err(ApiError::unavailable("github_joining_migration_required"));
        }
        Ok(())
    }
    pub fn notify_dispatch(&self) {
        self.dispatch.notify_one();
    }
    pub async fn wait_for_dispatch(&self) {
        tokio::select! {_ = self.dispatch.notified()=>{},_ = tokio::time::sleep(Duration::from_secs(30))=>{}}
    }
    fn rollout(&self, org: &str) -> Result<(), ApiError> {
        if !self.organizations.contains(org) {
            return Err(ApiError::forbidden("organization_workspaces_disabled"));
        }
        Ok(())
    }
    async fn lock<'a>(
        &'a self,
        org: &str,
    ) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, ApiError> {
        // Waiters release their pool connection between attempts. Holding a
        // connection waiting on an org lock could starve the lock holder's
        // nested complete-roster inspection and deadlock a small API pool.
        for _ in 0..200 {
            let mut tx = self.pool.begin().await?;
            let acquired: bool =
                sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("organization:{org}"))
                    .fetch_one(&mut *tx)
                    .await?;
            if acquired {
                return Ok(tx);
            }
            tx.rollback().await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err(ApiError::conflict("organization_operation_busy"))
    }
    async fn enrollment(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: &str,
        actor: &str,
    ) -> Result<Option<Enrollment>, ApiError> {
        let row:Option<Value>=sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id=$1 AND account_id=$2 FOR UPDATE").bind(org).bind(actor).fetch_optional(&mut **tx).await?;
        row.map(serde_json::from_value)
            .transpose()
            .map_err(|_| ApiError::unavailable("github_enrollment_invalid"))
    }
    async fn save(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        value: &Enrollment,
    ) -> Result<(), ApiError> {
        let events: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_account_event_outbox') IS NOT NULL")
                .fetch_one(&mut **tx)
                .await?;
        let previous: Option<Value> = if events {
            sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id=$1 AND account_id=$2 FOR UPDATE").bind(&value.organization_id).bind(&value.account_id).fetch_optional(&mut **tx).await?
        } else {
            None
        };
        sqlx::query("INSERT INTO api_github_enrollments(organization_id,account_id,installation_id,data) VALUES($1,$2,$3,$4) ON CONFLICT(organization_id,account_id) DO UPDATE SET installation_id=EXCLUDED.installation_id,data=EXCLUDED.data").bind(&value.organization_id).bind(&value.account_id).bind(value.installation_id).bind(json!(value)).execute(&mut **tx).await?;
        if events
            && value.reserved_until == 0
            && value.member_id.is_some()
            && previous.as_ref().is_none_or(|old| {
                old["memberId"] != json!(value.member_id)
                    || old["blocked"] != value.blocked
                    || old["reservedUntil"].as_i64() != Some(0)
            })
        {
            sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at) VALUES($1,'github',$2,$3,$4)").bind(&value.account_id).bind(format!("organization:{}",value.organization_id)).bind(json!({"organizationId":value.organization_id,"membershipChanged":true,"blocked":value.blocked})).bind(chrono::Utc::now().timestamp_millis()).execute(&mut **tx).await?;
        }
        Ok(())
    }
    async fn policy(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: &str,
        installation: i64,
    ) -> Result<Policy, ApiError> {
        let value:Value=sqlx::query_scalar("SELECT data FROM api_github_join_policies WHERE organization_id=$1 AND installation_id=$2 FOR UPDATE").bind(org).bind(installation).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::forbidden("organization_access_denied"))?;
        let policy: Policy = serde_json::from_value(value)
            .map_err(|_| ApiError::unavailable("github_policy_invalid"))?;
        if policy.organization_id != org || policy.installation_id != installation {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        Ok(policy)
    }
    async fn capacity(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        org: &str,
        actor: &str,
        members: &[Member],
        now: i64,
    ) -> Result<(), ApiError> {
        let active: BTreeSet<_> = members
            .iter()
            .filter(|m| m.organization_id == org && m.status == "active")
            .map(|m| m.user_id.as_str())
            .collect();
        let reservations: Vec<Value> =
            sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id=$1")
                .bind(org)
                .fetch_all(&mut **tx)
                .await?;
        let mut reserved = BTreeSet::new();
        for value in reservations {
            let e: Enrollment = serde_json::from_value(value)
                .map_err(|_| ApiError::unavailable("github_enrollment_invalid"))?;
            if !e.blocked
                && e.account_id != actor
                && !active.contains(e.account_id.as_str())
                && e.reserved_until > now
            {
                reserved.insert(e.account_id);
            }
        }
        let invites = self.directory.invitations(org).await?;
        let pending = invites
            .iter()
            .filter(|i| {
                i.organization_id.as_deref() == Some(org)
                    && i.state == "pending"
                    && chrono::DateTime::parse_from_rfc3339(&i.expires_at)
                        .map_or(true, |t| t.timestamp_millis() > now)
            })
            .count();
        if active.len() + reserved.len() + pending >= 5 {
            return Err(ApiError::conflict("organization_member_limit_reached"));
        }
        Ok(())
    }
    pub async fn join(
        &self,
        actor: &str,
        github_user_id: i64,
        org: &str,
        installation: i64,
        now: i64,
    ) -> Result<bool, ApiError> {
        self.rollout(org)?;
        // Two transactions intentionally make the seat reservation survive an API
        // crash while WorkOS is allocating. The second rechecks policy and capacity.
        let mut tx = self.lock(org).await?;
        let policy = Self::policy(&mut tx, org, installation).await?;
        let old = Self::enrollment(&mut tx, org, actor).await?;
        if !policy.enabled
            || old
                .as_ref()
                .is_some_and(|e| e.blocked || e.github_user_id != github_user_id)
            || !self
                .roster
                .members(&policy)
                .await?
                .contains(&github_user_id)
        {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        let members = self.directory.members(org).await?;
        let own: Vec<_> = members
            .iter()
            .filter(|m| m.organization_id == org && m.user_id == actor)
            .collect();
        if let Some(active) = own.iter().find(|m| m.status == "active") {
            // A lost WorkOS reply may have created the membership before the
            // final SQL commit. Reconcile the existing durable reservation.
            if let Some(mut enrollment) = old {
                enrollment.member_id = Some(active.id.clone());
                enrollment.reserved_until = 0;
                Self::save(&mut tx, &enrollment).await?;
                tx.commit().await?;
            }
            return Ok(false);
        }
        if old.is_none() && !own.is_empty() {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        self.capacity(&mut tx, org, actor, &members, now).await?;
        let reservation = Enrollment {
            organization_id: org.into(),
            account_id: actor.into(),
            github_user_id,
            installation_id: installation,
            github_org_id: policy.github_org_id,
            blocked: false,
            member_id: old.and_then(|e| e.member_id),
            reserved_until: now + 600000,
            revision: uuid::Uuid::new_v4().to_string(),
        };
        Self::save(&mut tx, &reservation).await?;
        tx.commit().await?;
        let mut tx = self.lock(org).await?;
        let mut enrollment = Self::enrollment(&mut tx, org, actor)
            .await?
            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))?;
        let current = Self::policy(&mut tx, org, installation).await?;
        if enrollment.blocked
            || enrollment.revision != reservation.revision
            || enrollment.installation_id != installation
            || current != policy
            || !current.enabled
            || !self
                .roster
                .members(&current)
                .await?
                .contains(&github_user_id)
        {
            return Err(ApiError::forbidden("organization_access_denied"));
        }
        let members = self.directory.members(org).await?;
        let own: Vec<_> = members
            .iter()
            .filter(|m| m.organization_id == org && m.user_id == actor)
            .collect();
        let (member, created) = if let Some(active) = own.iter().find(|m| m.status == "active") {
            ((*active).clone(), false)
        } else {
            self.capacity(&mut tx, org, actor, &members, now).await?;
            let previous = own.iter().find(|m| {
                m.status == "inactive" && !m.directory_managed && m.role.slug == "member"
            });
            (
                self.directory
                    .admit(org, actor, previous.map(|m| m.id.as_str()))
                    .await?,
                true,
            )
        };
        if member.organization_id != org
            || member.user_id != actor
            || member.status != "active"
            || (created && member.role.slug != "member")
        {
            return Err(ApiError::unavailable("organizations_invalid_response"));
        }
        enrollment.member_id = Some(member.id);
        enrollment.reserved_until = 0;
        Self::save(&mut tx, &enrollment).await?;
        tx.commit().await?;
        Ok(created)
    }
    /// Called only after browser-bound OAuth confirmation. The unique numeric ID
    /// constraint prevents the same GitHub identity enrolling two Zuse accounts.
    #[cfg(test)]
    pub(super) async fn save_verified_identity(
        &self,
        identity: &VerifiedIdentity,
    ) -> Result<(), ApiError> {
        if identity.github_user_id <= 0
            || identity.account_id.is_empty()
            || identity.verification_id.is_empty()
        {
            return Err(ApiError::bad_request("github_identity_invalid"));
        }
        let result=sqlx::query("INSERT INTO api_github_identities(account_id,github_user_id,data) VALUES($1,$2,$3) ON CONFLICT(account_id) DO UPDATE SET github_user_id=EXCLUDED.github_user_id,data=EXCLUDED.data").bind(&identity.account_id).bind(identity.github_user_id).bind(json!(identity)).execute(&self.pool).await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(ApiError::conflict("github_identity_already_linked"))
            }
            Err(e) => Err(e.into()),
        }
    }
    pub async fn sync(&self, actor: &str, now: i64) -> Result<Vec<String>, ApiError> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT github_user_id FROM api_github_identities WHERE account_id=$1",
        )
        .bind(actor)
        .fetch_optional(&self.pool)
        .await?;
        let Some(id) = id else { return Ok(vec![]) };
        let policies:Vec<Value>=sqlx::query_scalar("SELECT DISTINCT p.data FROM api_github_join_policies p JOIN api_github_org_members r USING(installation_id) WHERE r.github_user_id=$1").bind(id).fetch_all(&self.pool).await?;
        let mut joined = vec![];
        for value in policies {
            let p: Policy = serde_json::from_value(value)
                .map_err(|_| ApiError::unavailable("github_policy_invalid"))?;
            if p.enabled
                && self.organizations.contains(&p.organization_id)
                && !joined.contains(&p.organization_id)
                && self
                    .join(actor, id, &p.organization_id, p.installation_id, now)
                    .await
                    .unwrap_or(false)
            {
                joined.push(p.organization_id);
            }
        }
        Ok(joined)
    }
    pub async fn restore(&self, access: &Access, actor: &str) -> Result<(), ApiError> {
        let org = admin(access)?;
        self.rollout(org)?;
        let mut tx = self.lock(org).await?;
        let mut enrollment = Self::enrollment(&mut tx, org, actor)
            .await?
            .ok_or_else(|| ApiError::not_found("github_enrollment_not_found"))?;
        if enrollment.blocked {
            for member in
                self.directory.members(org).await?.iter().filter(|m| {
                    m.user_id == actor && m.organization_id == org && m.status == "active"
                })
            {
                self.directory.deactivate(&member.id).await?;
            }
        }
        enrollment.blocked = false;
        enrollment.revision = uuid::Uuid::new_v4().to_string();
        Self::save(&mut tx, &enrollment).await?;
        tx.commit().await?;
        Ok(())
    }
}
fn admin(access: &Access) -> Result<&str, ApiError> {
    match (&access.scope, &access.membership) {
        (crate::access::Scope::Organization { organization_id }, Some(m))
            if m.status == "active"
                && m.user_id == access.actor.account_id
                && m.organization_id == *organization_id
                && m.role.slug == "admin"
                && access.owner_id == format!("organization:{organization_id}") =>
        {
            Ok(organization_id)
        }
        _ => Err(ApiError::forbidden("organization_admin_required")),
    }
}
