use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct FakeDirectory {
    members: Mutex<Vec<Member>>,
    invites: AtomicUsize,
    calls: AtomicUsize,
    lose: AtomicBool,
}
impl FakeDirectory {
    fn new(count: usize) -> Self {
        Self {
            members: Mutex::new(
                (0..count)
                    .map(|i| member(&format!("existing-{i}")))
                    .collect(),
            ),
            invites: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            lose: AtomicBool::new(false),
        }
    }
}
fn member(actor: &str) -> Member {
    Member {
        id: format!("membership-{actor}"),
        user_id: actor.into(),
        organization_id: "team".into(),
        status: "active".into(),
        directory_managed: false,
        role: crate::auth::MembershipRole {
            slug: "member".into(),
        },
    }
}
#[async_trait]
impl Directory for FakeDirectory {
    async fn members(&self, _: &str) -> Result<Vec<Member>, ApiError> {
        Ok(self.members.lock().unwrap().clone())
    }
    async fn invitations(&self, _: &str) -> Result<Vec<Invitation>, ApiError> {
        Ok((0..self.invites.load(Ordering::SeqCst))
            .map(|_| Invitation {
                organization_id: Some("team".into()),
                state: "pending".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
            })
            .collect())
    }
    async fn admit(
        &self,
        _: &str,
        actor: &str,
        previous: Option<&str>,
    ) -> Result<Member, ApiError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut member = member(actor);
        if let Some(id) = previous {
            member.id = id.into();
        }
        let mut members = self.members.lock().unwrap();
        members.retain(|m| m.user_id != actor);
        members.push(member.clone());
        if self.lose.swap(false, Ordering::SeqCst) {
            return Err(ApiError::unavailable("organizations_unavailable"));
        }
        Ok(member)
    }
    async fn deactivate(&self, id: &str) -> Result<(), ApiError> {
        for member in self
            .members
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|m| m.id == id)
        {
            member.status = "inactive".into();
        }
        Ok(())
    }
}
struct FakeRoster;
#[async_trait]
impl Roster for FakeRoster {
    async fn members(&self, _: &Policy) -> Result<BTreeSet<i64>, ApiError> {
        Ok([41, 42, 43, 44, 45, 46].into_iter().collect())
    }
}
async fn database() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("explicit isolated DB");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("join_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let scope = scope.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={scope}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for migration in [
        include_str!("../../../../migrations/0000_application.sql"),
        include_str!("../../../../migrations/0015_github.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    }
    let policy = Policy {
        organization_id: "team".into(),
        installation_id: 9,
        github_org_id: 81,
        login: "team".into(),
        enabled: true,
        revision: "policy-one".into(),
    };
    sqlx::query("INSERT INTO api_github_join_policies VALUES('team',9,81,$1)")
        .bind(json!(policy))
        .execute(&pool)
        .await
        .unwrap();
    (admin, pool, schema)
}
#[tokio::test]
#[ignore = "explicit isolated localhost PostgreSQL"]
async fn durable_join_seats_invitations_races_and_uncertain_workos_recovery() {
    let (admin, pool, schema) = database().await;
    let directory = Arc::new(FakeDirectory::new(3));
    directory.invites.store(1, Ordering::SeqCst);
    let joining = Joining::new(
        pool.clone(),
        directory.clone(),
        Arc::new(FakeRoster),
        vec!["team".into()],
    );
    let now = chrono::Utc::now().timestamp_millis();
    let (a, b) = tokio::join!(
        joining.join("one", 41, "team", 9, now),
        joining.join("two", 42, "team", 9, now)
    );
    assert_eq!(
        usize::from(a.as_ref().is_ok_and(|created| *created))
            + usize::from(b.as_ref().is_ok_and(|created| *created)),
        1
    );
    let error = if a.is_err() {
        a.unwrap_err()
    } else {
        b.unwrap_err()
    };
    assert_eq!(error.code, "organization_member_limit_reached");
    assert_eq!(directory.calls.load(Ordering::SeqCst), 1);
    directory.invites.store(0, Ordering::SeqCst);
    directory.lose.store(true, Ordering::SeqCst);
    assert_eq!(
        joining
            .join("lost", 43, "team", 9, now)
            .await
            .unwrap_err()
            .code,
        "organizations_unavailable"
    );
    assert!(!joining.join("lost", 43, "team", 9, now + 1).await.unwrap());
    let e:Value=sqlx::query_scalar("SELECT data FROM api_github_enrollments WHERE organization_id='team' AND account_id='lost'").fetch_one(&pool).await.unwrap();
    assert_eq!(e["memberId"], "membership-lost");
    assert_eq!(e["reservedUntil"], 0);
    assert_eq!(directory.calls.load(Ordering::SeqCst), 2);
    let identity = VerifiedIdentity {
        account_id: "one".into(),
        github_user_id: 41,
        login: "one".into(),
        organization_ids: vec![81],
        verification_id: "verified-browser".into(),
    };
    joining.save_verified_identity(&identity).await.unwrap();
    let mut collision = identity.clone();
    collision.account_id = "different-account".into();
    assert_eq!(
        joining
            .save_verified_identity(&collision)
            .await
            .unwrap_err()
            .code,
        "github_identity_already_linked"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
fn access(actor: &str, role: &str) -> Access {
    Access {
        actor: crate::auth::Principal {
            account_id: actor.into(),
        },
        scope: crate::access::Scope::Organization {
            organization_id: "team".into(),
        },
        owner_id: "organization:team".into(),
        membership: Some(crate::auth::Membership {
            id: format!("membership-{actor}"),
            user_id: actor.into(),
            organization_id: "team".into(),
            status: "active".into(),
            role: crate::auth::MembershipRole { slug: role.into() },
        }),
    }
}
#[tokio::test]
#[ignore = "explicit isolated localhost PostgreSQL"]
async fn managed_rejoin_and_admin_block_restore_preserve_original_membership_rules() {
    let (admin, pool, schema) = database().await;
    let directory = Arc::new(FakeDirectory::new(0));
    let joining = Joining::new(
        pool.clone(),
        directory.clone(),
        Arc::new(FakeRoster),
        vec!["team".into()],
    );
    let now = chrono::Utc::now().timestamp_millis();
    let mut prior = member("returning");
    prior.id = "original-membership".into();
    prior.status = "inactive".into();
    directory.members.lock().unwrap().push(prior.clone());
    assert_eq!(
        joining
            .join("returning", 44, "team", 9, now)
            .await
            .unwrap_err()
            .code,
        "organization_access_denied",
        "never self-reactivate a manually removed non-managed membership"
    );
    let e = Enrollment {
        organization_id: "team".into(),
        account_id: "returning".into(),
        github_user_id: 44,
        installation_id: 9,
        github_org_id: 81,
        blocked: false,
        member_id: Some(prior.id.clone()),
        reserved_until: 0,
        revision: "old-enrollment".into(),
    };
    let mut tx = pool.begin().await.unwrap();
    Joining::save(&mut tx, &e).await.unwrap();
    tx.commit().await.unwrap();
    assert!(joining.join("returning", 44, "team", 9, now).await.unwrap());
    let members = directory.members.lock().unwrap().clone();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, "original-membership");
    assert_eq!(
        joining
            .block(&access("ordinary", "member"), "returning")
            .await
            .unwrap_err()
            .code,
        "organization_admin_required"
    );
    joining
        .block(&access("administrator", "admin"), "returning")
        .await
        .unwrap();
    assert_eq!(
        joining
            .join("returning", 44, "team", 9, now)
            .await
            .unwrap_err()
            .code,
        "organization_access_denied"
    );
    joining
        .restore(&access("administrator", "admin"), "returning")
        .await
        .unwrap();
    assert!(joining.join("returning", 44, "team", 9, now).await.unwrap());
    let mut directory_member = member("directory-user");
    directory_member.status = "inactive".into();
    directory_member.directory_managed = true;
    directory
        .members
        .lock()
        .unwrap()
        .push(directory_member.clone());
    let mut directory_enrollment = e.clone();
    directory_enrollment.account_id = "directory-user".into();
    directory_enrollment.github_user_id = 45;
    directory_enrollment.member_id = Some(directory_member.id);
    let mut tx = pool.begin().await.unwrap();
    Joining::save(&mut tx, &directory_enrollment).await.unwrap();
    tx.commit().await.unwrap();
    joining
        .join("directory-user", 45, "team", 9, now)
        .await
        .unwrap();
    assert_ne!(
        directory
            .members
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.user_id == "directory-user")
            .unwrap()
            .status,
        "inactive"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
struct FailingRoster;
#[async_trait]
impl Roster for FailingRoster {
    async fn members(&self, _: &Policy) -> Result<BTreeSet<i64>, ApiError> {
        Err(ApiError::unavailable("github_members_unavailable"))
    }
}
#[tokio::test]
#[ignore = "explicit isolated localhost PostgreSQL"]
async fn durable_join_jobs_coalesce_recover_expired_leases_and_do_not_repeat_admission() {
    let (admin, pool, schema) = database().await;
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0018_github_joining_jobs.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let directory = Arc::new(FakeDirectory::new(0));
    let joining = Joining::new(
        pool.clone(),
        directory.clone(),
        Arc::new(FakeRoster),
        vec!["team".into()],
    );
    let now = chrono::Utc::now().timestamp_millis();
    let identity = VerifiedIdentity {
        account_id: "one".into(),
        github_user_id: 41,
        login: "one".into(),
        organization_ids: vec![81],
        verification_id: "browser-proof".into(),
    };
    joining.save_verified_identity(&identity).await.unwrap();
    sqlx::query("INSERT INTO api_github_org_members VALUES(9,41)")
        .execute(&pool)
        .await
        .unwrap();
    joining.schedule(9, now).await.unwrap();
    joining.schedule(9, now).await.unwrap();
    sqlx::query("UPDATE zuse_rust_github_join_jobs SET lease_id='old-worker',lease_until=$1")
        .bind(now + 1000)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!joining.run_once(now).await.unwrap());
    assert!(joining.run_once(now + 1001).await.unwrap());
    assert_eq!(directory.calls.load(Ordering::SeqCst), 1);
    let completed: i64 =
        sqlx::query_scalar("SELECT completed_version FROM zuse_rust_github_join_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(completed, 2);
    joining.schedule(9, now + 2000).await.unwrap();
    let failure = Joining::new(
        pool.clone(),
        directory.clone(),
        Arc::new(FailingRoster),
        vec!["team".into()],
    );
    assert!(failure.run_once(now + 2000).await.unwrap());
    let (code, completed): (String, i64) =
        sqlx::query_as("SELECT last_error,completed_version FROM zuse_rust_github_join_jobs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(code, "github_members_unavailable");
    assert_eq!(completed, 2);
    assert_eq!(
        directory.members.lock().unwrap()[0].status,
        "active",
        "failed roster must not remove a member"
    );
    sqlx::query("UPDATE zuse_rust_github_join_jobs SET next_attempt_at=0")
        .execute(&pool)
        .await
        .unwrap();
    joining.run_once(now + 3000).await.unwrap();
    assert_eq!(
        directory.calls.load(Ordering::SeqCst),
        1,
        "recovery rechecks existing active membership"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
#[tokio::test]
#[ignore = "explicit isolated localhost PostgreSQL"]
async fn joining_restart_seeds_scoped_jobs_and_final_membership_events_target_only_the_actor() {
    let (admin, pool, schema) = database().await;
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0018_github_joining_jobs.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let first_outbox = include_str!("../../../../migrations/0017_account_events.sql")
        .split("CREATE TABLE zuse_rust_account_event_cursors")
        .next()
        .unwrap();
    sqlx::raw_sql(first_outbox).execute(&pool).await.unwrap();
    let directory = Arc::new(FakeDirectory::new(0));
    let joining = Joining::new(
        pool.clone(),
        directory.clone(),
        Arc::new(FakeRoster),
        vec!["team".into()],
    );
    let now = chrono::Utc::now().timestamp_millis();
    let other = Policy {
        organization_id: "off-cohort".into(),
        installation_id: 10,
        github_org_id: 82,
        login: "other".into(),
        enabled: true,
        revision: "other".into(),
    };
    sqlx::query("INSERT INTO api_github_join_policies VALUES('off-cohort',10,82,$1)")
        .bind(json!(other))
        .execute(&pool)
        .await
        .unwrap();
    joining.seed(now).await.unwrap();
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT installation_id FROM zuse_rust_github_join_jobs")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids, vec![9]);
    assert!(joining.join("one", 41, "team", 9, now).await.unwrap());
    let (owner, kind, payload): (String, String, Value) =
        sqlx::query_as("SELECT owner_id,kind,payload FROM zuse_rust_account_event_outbox")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owner, "one");
    assert_eq!(kind, "github");
    assert_eq!(
        payload,
        json!({"organizationId":"team","membershipChanged":true,"blocked":false})
    );
    assert!(!joining.join("one", 41, "team", 9, now).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_account_event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "re-reading the same active membership does not publish a duplicate event"
    );
    joining
        .block(&access("administrator", "admin"), "one")
        .await
        .unwrap();
    let(owner,payload):(String,Value)=sqlx::query_as("SELECT owner_id,payload FROM zuse_rust_account_event_outbox ORDER BY outbox_id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(owner, "one");
    assert_eq!(payload["blocked"], true);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
