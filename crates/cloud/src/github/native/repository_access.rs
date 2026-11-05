//! Repository eligibility uses the same selected-user/App intersection as Git grants.
//! Admission metadata carries no token and is rechecked before compute is allocated.
use super::*;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepositoryAuthorization {
    owner: String,
    actor: String,
    repository: String,
    installation: i64,
    scope_epoch: i64,
    user_epoch: i64,
    github_user_id: i64,
}

pub(crate) fn repository_from_url(value: &str) -> Result<String, ApiError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| ApiError::bad_request("invalid_github_repository"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad_request("invalid_github_repository"));
    }
    let path = url.path().trim_start_matches('/');
    let repository = path.strip_suffix(".git").unwrap_or(path);
    if !repository
        .split_once('/')
        .is_some_and(|(owner, name)| valid_component(owner) && valid_component(name))
    {
        return Err(ApiError::bad_request("invalid_github_repository"));
    }
    Ok(repository.to_owned())
}

pub(crate) fn mark_project_access(projects: &mut [Value], catalog: &GithubCatalog) {
    let allowed: BTreeSet<String> = catalog
        .repositories
        .iter()
        .map(|r| r.name_with_owner.to_ascii_lowercase())
        .collect();
    for project in projects {
        let can_create = project["repositoryUrl"]
            .as_str()
            .and_then(|url| repository_from_url(url).ok())
            .is_some_and(|repo| allowed.contains(&repo.to_ascii_lowercase()));
        project["canCreate"] = json!(can_create);
        project["repositoryAccess"] = json!(if can_create {
            "authorized"
        } else if catalog.user.is_none() {
            "github_user_reconnect_required"
        } else {
            "github_repository_access_denied"
        });
    }
}

// A concurrent metadata observer can supersede a nonce without changing access.
// Retry only that explicit transient result; revocation/identity failures stay final.
async fn retry_observation<T, F, Fut>(mut observe: F) -> Result<T, ApiError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    for attempt in 0..3 {
        let result = observe().await;
        if attempt < 2
            && result
                .as_ref()
                .err()
                .is_some_and(|error| error.code == "github_observation_superseded")
        {
            tokio::time::sleep(Duration::from_millis(10 * (attempt + 1))).await;
            continue;
        }
        return result;
    }
    unreachable!("bounded observation returns its last result")
}

impl NativeGithub {
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%owner, operation="github_installation"))]
    pub(super) async fn repository_installation(
        &self,
        owner: &str,
        repository: &str,
        token: &str,
    ) -> Result<(i64, i64), ApiError> {
        let (scope_epoch, links) = self.repository_links(owner, repository).await?;
        self.verify_repository_links(owner, repository, token, scope_epoch, links).await
    }
    pub(super) async fn repository_links(&self, owner: &str, repository: &str) -> Result<(i64, Vec<sqlx::postgres::PgRow>), ApiError> {
        let (target, _) = repository
            .split_once('/')
            .filter(|(a, b)| valid_component(a) && valid_component(b))
            .ok_or_else(|| ApiError::bad_request("invalid_github_repository"))?;
        // Read scope epoch and active links in one statement/snapshot. The final
        // transaction still rechecks the current scope, user and allowlist fences.
        let links=sqlx::query("SELECT s.epoch,i.installation_id,i.metadata,i.allowed_repositories FROM zuse_rust_github_scopes s LEFT JOIN zuse_rust_github_installations i ON i.owner_id=s.owner_id AND i.state='active' WHERE s.owner_id=$1 ORDER BY i.installation_id").bind(owner).fetch_all(&self.pool).await?;
        let scope_epoch: i64 = links
            .first()
            .ok_or_else(|| ApiError::forbidden("github_repository_access_denied"))?
            .try_get("epoch")?;
        if !links.iter().any(|row| row.try_get::<Value,_>("metadata").ok().is_some_and(|metadata|
            metadata["account"]["login"].as_str().is_some_and(|login| login.eq_ignore_ascii_case(target)))) {
            return Err(ApiError::forbidden("github_repository_access_denied"));
        }
        Ok((scope_epoch, links))
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%owner, operation="github_installation_verification"))]
    pub(super) async fn verify_repository_links(&self, owner: &str, repository: &str, token: &str, scope_epoch: i64, links: Vec<sqlx::postgres::PgRow>) -> Result<(i64, i64), ApiError> {
        let (target, _) = repository.split_once('/').ok_or_else(|| ApiError::bad_request("invalid_github_repository"))?;
        for row in links {
            let Some(id) = row.try_get::<Option<i64>, _>("installation_id")? else {
                continue;
            };
            let metadata: Value = row.try_get("metadata")?;
            if !metadata["account"]["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(target))
            {
                continue;
            }
            // Both reads concern this already-linked installation and are
            // independent: App observation uses its signing identity; repository
            // permissions use the selected user's existing credential. Neither
            // grants execution access. Await BOTH, then retain observation-first
            // failure precedence and all existing nonce/identity/revocation fences.
            let (observation, repositories) =
                tokio::join!(self.observe(owner, id), self.repositories(id, token),);
            let Some(fresh) = observation? else {
                continue;
            };
            let allowed: Option<Value> = row.try_get("allowed_repositories")?;
            if repositories?.iter().any(|repo| {
                repo["full_name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(repository))
                    && repository_allowed(&fresh, allowed.as_ref(), repo)
            }) {
                return Ok((id, scope_epoch));
            }
        }
        Err(ApiError::forbidden("github_repository_access_denied"))
    }
    /// Validate repository access without minting an execution credential. HTTP
    /// observation happens before the caller's allocation transaction begins.
    pub(crate) async fn authorize_repository(
        &self,
        owner: &str,
        actor: &str,
        url: &str,
    ) -> Result<RepositoryAuthorization, ApiError> {
        // Called before creation opens its account/admission transaction. The
        // nonce, credential binding and revocation fences run anew each time.
        retry_observation(|| self.authorize_repository_once(owner, actor, url)).await
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%owner, operation="github_authorization"))]
    async fn authorize_repository_once(
        &self,
        owner: &str,
        actor: &str,
        url: &str,
    ) -> Result<RepositoryAuthorization, ApiError> {
        if !self.enabled(owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        let repository = repository_from_url(url)?;
        let credentials = self.credentials(actor).await?;
        let before = self
            .credential_binding(actor, &credentials.access_token)
            .await?;
        let (installation, scope_epoch) = self
            .repository_installation(owner, &repository, &credentials.access_token)
            .await?;
        let after = self
            .credential_binding(actor, &credentials.access_token)
            .await?;
        if before != after {
            return Err(reconnect());
        }
        let proof = RepositoryAuthorization {
            owner: owner.to_owned(),
            actor: actor.to_owned(),
            repository,
            installation,
            scope_epoch,
            user_epoch: after.0,
            github_user_id: after.2["id"]
                .as_i64()
                .filter(|n| *n > 0)
                .ok_or_else(unavailable)?,
        };
        let mut tx = self.pool.begin().await?;
        self.revalidate_repository(&mut tx, &proof, url).await?;
        tx.commit().await?;
        Ok(proof)
    }
    #[tracing::instrument(name="cloud_admission_stage", skip_all, fields(owner=%proof.owner, operation="github_revalidation"))]
    pub(crate) async fn revalidate_repository(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        proof: &RepositoryAuthorization,
        url: &str,
    ) -> Result<(), ApiError> {
        if !self.enabled(&proof.owner)
            || !repository_from_url(url)?.eq_ignore_ascii_case(&proof.repository)
        {
            return Err(ApiError::forbidden("github_repository_access_denied"));
        }
        let (target, _) = proof.repository.split_once('/').ok_or_else(unavailable)?;
        let allowed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_github_installations i JOIN zuse_rust_github_scopes s USING(owner_id) JOIN zuse_rust_github_users u ON u.actor_id=$4 WHERE i.owner_id=$1 AND i.installation_id=$2 AND i.state='active' AND s.epoch=$3 AND u.epoch=$5 AND u.github_user_id=$6 AND u.state='connected' AND NOT u.refresh_uncertain AND lower(i.metadata#>>'{account,login}')=lower($7) AND (i.metadata->'suspended_at' IS NULL OR i.metadata->'suspended_at'='null'::jsonb) AND CASE WHEN i.allowed_repositories IS NULL THEN true WHEN jsonb_typeof(i.allowed_repositories)='array' THEN EXISTS(SELECT 1 FROM jsonb_array_elements_text(i.allowed_repositories) AS allowed(name) WHERE lower(allowed.name)=lower($8)) ELSE false END)")
            .bind(&proof.owner).bind(proof.installation).bind(proof.scope_epoch).bind(&proof.actor).bind(proof.user_epoch).bind(proof.github_user_id).bind(target).bind(&proof.repository).fetch_one(&mut **tx).await?;
        if !allowed {
            return Err(ApiError::forbidden("github_repository_access_denied"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn superseded_observation_retries_are_bounded_and_do_not_retry_denials() {
        let calls = AtomicUsize::new(0);
        let success = super::retry_observation(|| async {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(ApiError::conflict("github_observation_superseded"))
            } else {
                Ok(7)
            }
        })
        .await
        .unwrap();
        assert_eq!(success, 7);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        calls.store(0, Ordering::SeqCst);
        let exhausted = super::retry_observation::<(), _, _>(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(ApiError::conflict("github_observation_superseded"))
        })
        .await
        .unwrap_err();
        assert_eq!(exhausted.code, "github_observation_superseded");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        for code in [
            "github_reconnect_required",
            "github_repository_access_denied",
            "github_native_disabled",
        ] {
            calls.store(0, Ordering::SeqCst);
            let denied = super::retry_observation::<(), _, _>(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ApiError::forbidden(code))
            })
            .await
            .unwrap_err();
            assert_eq!(denied.code, code);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
    use super::*;
    #[test]
    fn retained_project_metadata_is_preserved_but_only_authorized_repositories_can_create() {
        let mut projects = vec![
            json!({"projectId":"retained","repositoryUrl":"https://github.com/octocat/Hello-World.git","displayName":"Retained project"}),
            json!({"projectId":"selected","repositoryUrl":"https://github.com/Team/Project.git"}),
        ];
        let catalog = GithubCatalog {
            configured: true,
            can_manage_installations: Some(false),
            user: Some(GithubUser {
                login: "member".into(),
                name: "Member".into(),
                email: "42+member@users.noreply.github.com".into(),
            }),
            installations: vec![],
            repositories: vec![GithubRepository {
                name_with_owner: "team/project".into(),
                description: None,
                ssh_url: "git@github.com:team/project.git".into(),
                https_url: "https://github.com/team/project.git".into(),
                is_private: true,
                default_branch: "main".into(),
                updated_at: "".into(),
                owner_avatar_url: None,
            }],
        };
        mark_project_access(&mut projects, &catalog);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0]["projectId"], "retained");
        assert_eq!(projects[0]["displayName"], "Retained project");
        assert_eq!(projects[0]["canCreate"], false);
        assert_eq!(
            projects[0]["repositoryAccess"],
            "github_repository_access_denied"
        );
        assert_eq!(projects[1]["canCreate"], true);
    }
    #[test]
    fn repository_urls_cannot_smuggle_tokens_or_different_origins() {
        for url in [
            "https://token@github.com/team/project.git",
            "https://github.com.evil.test/team/project.git",
            "https://github.com/team/project.git?token=bad",
            "https://github.com/team/project/more",
        ] {
            assert!(repository_from_url(url).is_err());
        }
        assert_eq!(
            repository_from_url("https://github.com/team/project.git").unwrap(),
            "team/project"
        );
    }
    #[tokio::test]
    #[ignore = "requires isolated localhost PostgreSQL"]
    async fn launch_preflight_reuses_repository_authorization_without_minting_tokens_and_fences_revocation()
     {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("github_launch_access_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let scope = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
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
        sqlx::raw_sql(include_str!("../../../migrations/0015_github.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let metadata = json!({"id":9,"account":{"id":81,"login":"team","type":"Organization"},"repository_selection":"all","suspended_at":null});
        let observed = metadata.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let repository_reads = reads.clone();
        let tokens = Arc::new(AtomicUsize::new(0));
        let token_requests = tokens.clone();
        let overlap = Arc::new(tokio::sync::Barrier::new(2));
        let app_overlap = overlap.clone();
        let pair_reads = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let app_pair_reads = pair_reads.clone();
        let repository_overlap = overlap.clone();
        let revoke_during_reads = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let revoke = revoke_during_reads.clone();
        let observed_pool = pool.clone();
        let app = Router::new()
            .route("/app/installations/9", get(move || {
                let observed = observed.clone();
                let overlap = app_overlap.clone();
                let pair_reads = app_pair_reads.clone();
                async move {
                    if pair_reads.load(Ordering::SeqCst) {
                        tokio::time::timeout(Duration::from_secs(2), overlap.wait()).await.expect("installation and user-permission HTTP reads must overlap");
                    } else {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Json(observed)
                }
            }))
            .route("/user/installations/9/repositories", get(move || {
                repository_reads.fetch_add(1, Ordering::SeqCst);
                let overlap = repository_overlap.clone();
                let revoke = revoke.clone();
                let pool = observed_pool.clone();
                async move {
                    tokio::time::timeout(Duration::from_secs(2), overlap.wait()).await.expect("user-permission read must begin before installation read finishes");
                    if revoke.swap(false, Ordering::SeqCst) {
                        sqlx::query("UPDATE zuse_rust_github_scopes SET epoch=epoch+1 WHERE owner_id='fixture-actor'").execute(&pool).await.unwrap();
                    }
                    Json(json!({"repositories":[{"full_name":"team/project","permissions":{"push":true},"private":true,"default_branch":"main"}]}))
                }
            }))
            .route("/applications/client/token/scoped", post(move || {
                token_requests.fetch_add(1, Ordering::SeqCst);
                async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":"must-not-mint"}))) }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let der = key.to_pkcs1_der().unwrap();
        let authority = NativeGithub {
            membership_epoch: std::sync::atomic::AtomicU64::new(0),
            observation_gates: Default::default(),
            fast_start_accounts: Default::default(),
            pool: pool.clone(),
            master: MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
            accounts: BTreeSet::from(["fixture-actor".into()]),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            config: Config {
                origin: endpoint.clone(),
                app_id: "123".into(),
                slug: "fixture".into(),
                client_id: "client".into(),
                client_secret: Zeroizing::new("fixture-secret".into()),
                signing_key: jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()),
                webhook_secret: Zeroizing::new("fixture-hook".into()),
                api_origin: endpoint.clone(),
                oauth_origin: endpoint,
            },
        };
        let sealed = authority
            .seal(
                "github-user:fixture-actor:1",
                &Credentials {
                    access_token: "fixture-user-token".into(),
                    expires_at: None,
                    refresh_token: None,
                    refresh_expires_at: None,
                },
            )
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_github_users(actor_id,github_user_id,profile,sealed_credentials,state,epoch,version,updated_at)VALUES('fixture-actor',42,'{\"id\":42,\"login\":\"member\"}',$1,'connected',1,1,1)").bind(&sealed).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id,epoch)VALUES('fixture-actor',1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,state,updated_at)VALUES('fixture-actor',9,$1,'active',1)").bind(metadata).execute(&pool).await.unwrap();
        assert_eq!(
            authority
                .authorize_repository(
                    "fixture-actor",
                    "fixture-actor",
                    "https://github.com/octocat/Hello-World.git"
                )
                .await
                .unwrap_err()
                .code,
            "github_repository_access_denied"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(tokens.load(Ordering::SeqCst), 0);
        let proof = authority
            .authorize_repository(
                "fixture-actor",
                "fixture-actor",
                "https://github.com/team/project.git",
            )
            .await
            .unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            tokens.load(Ordering::SeqCst),
            0,
            "launch preflight must not mint a token"
        );
        // Actual admission observers for the same installation must not replace
        // one another's nonce. Each still fetches and validates fresh metadata.
        pair_reads.store(false, Ordering::SeqCst);
        let observations =
            futures::future::join_all((0..10).map(|_| authority.observe("fixture-actor", 9))).await;
        assert!(
            observations
                .iter()
                .all(|result| result.as_ref().is_ok_and(Option::is_some)),
            "concurrent admission observers superseded each other: {:?}",
            observations
                .iter()
                .map(|result| result.as_ref().err().map(|error| error.code))
                .collect::<Vec<_>>()
        );
        pair_reads.store(true, Ordering::SeqCst);
        let mut tx = pool.begin().await.unwrap();
        authority
            .revalidate_repository(&mut tx, &proof, "https://github.com/team/project.git")
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        sqlx::query(
            "UPDATE zuse_rust_github_installations SET allowed_repositories='[\"team/other\"]'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(
            authority
                .revalidate_repository(&mut tx, &proof, "https://github.com/team/project.git")
                .await
                .unwrap_err()
                .code,
            "github_repository_access_denied"
        );
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE zuse_rust_github_installations SET allowed_repositories=NULL")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE zuse_rust_github_scopes SET epoch=epoch+1")
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(
            authority
                .revalidate_repository(&mut tx, &proof, "https://github.com/team/project.git")
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
        // Revocation during the overlapping HTTP reads must fail the final
        // epoch check, even though both remote responses otherwise allow access.
        revoke_during_reads.store(true, Ordering::SeqCst);
        assert_eq!(
            authority
                .authorize_repository(
                    "fixture-actor",
                    "fixture-actor",
                    "https://github.com/team/project.git"
                )
                .await
                .unwrap_err()
                .code,
            "github_repository_access_denied"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        assert_eq!(tokens.load(Ordering::SeqCst), 0);
        let unchanged:(String,i64)=sqlx::query_as("SELECT sealed_credentials,version FROM zuse_rust_github_users WHERE actor_id='fixture-actor'").fetch_one(&pool).await.unwrap();
        assert_eq!(unchanged, (sealed, 1));
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
