//! Real authority records and HTTP observations for isolated admission fixtures.
//! These synthetic credentials never authorize a real GitHub request.
use super::*;
use axum::{Json, Router, routing::get};
use rsa::pkcs1::EncodeRsaPrivateKey;

pub(crate) struct Fixture {
    pub github: Arc<NativeGithub>,
    pub origin: String,
    pub reads: Arc<std::sync::atomic::AtomicUsize>,
    _server: tokio_util::task::AbortOnDropHandle<()>,
}

pub(crate) struct PersonalIdentity;
#[async_trait::async_trait]
impl crate::auth::Identity for PersonalIdentity {
    async fn verify(&self, _: &str) -> Result<crate::auth::Principal, ApiError> {
        unreachable!("admission supplies an already authenticated principal")
    }
    async fn membership(&self, _: &str, _: &str) -> Result<crate::auth::Membership, ApiError> {
        unreachable!("personal admission does not check organization membership")
    }
}

pub(crate) async fn connected(pool: &PgPool, owner: &str, actor: &str) -> Fixture {
    connected_mode(pool, owner, actor, false).await
}
pub(crate) async fn connected_for_preparation(pool: &PgPool, owner: &str, actor: &str) -> Fixture {
    connected_mode(pool, owner, actor, true).await
}
async fn connected_mode(pool: &PgPool, owner: &str, actor: &str, source: bool) -> Fixture {
    let metadata = json!({"id":9,"account":{"id":81,"login":"example","type":"Organization"},"repository_selection":"all","suspended_at":null});
    let observed = metadata.clone();
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let app_reads = reads.clone();
    let repository_reads = reads.clone();
    let mut app = Router::new()
        .route("/app/installations/9", get(move || {
            app_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let metadata = observed.clone();
            async move { Json(metadata) }
        }))
        .route("/user/installations/9/repositories", get(move || {
            repository_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Json(json!({"repositories":[{"full_name":"example/repo","permissions":{"push":true},"private":true,"default_branch":"main"}]})) }
        }));
    // Ordinary admission has no exchange endpoint: it must not mint a grant.
    if source {
        app = app.route("/applications/client/token/scoped", axum::routing::post(|| async {
            Json(json!({"token":"synthetic-scoped-preparation-token","expires_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()}))
        })).route("/repos/example/repo", get(|| async {
            Json(json!({"id":77,"private":true,"full_name":"example/repo"}))
        })).route("/repos/example/repo/commits/main", get(|| async {
            Json(json!({"sha":"c".repeat(40)}))
        })).route("/repos/example/repo/contents/.zuse/setup.json", get(|| async {
            axum::http::StatusCode::NOT_FOUND
        }));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap()
    }));
    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let der = key.to_pkcs1_der().unwrap();
    let github = Arc::new(NativeGithub {
        membership_epoch: std::sync::atomic::AtomicU64::new(0),
        observation_gates: Default::default(),
            fast_start_accounts: Default::default(),
        pool: pool.clone(),
        master: MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
        accounts: BTreeSet::from([owner.to_owned(), actor.to_owned()]),
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
            oauth_origin: endpoint.clone(),
        },
    });
    let sealed = github
        .seal(
            &format!("github-user:{actor}:1"),
            &Credentials {
                access_token: "synthetic-user-token".into(),
                expires_at: None,
                refresh_token: None,
                refresh_expires_at: None,
            },
        )
        .unwrap();
    sqlx::query("INSERT INTO zuse_rust_github_users(actor_id,github_user_id,profile,sealed_credentials,state,epoch,version,updated_at) VALUES($1,42,'{\"id\":42,\"login\":\"member\"}',$2,'connected',1,1,1)").bind(actor).bind(sealed).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id,epoch) VALUES($1,1)")
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,state,updated_at) VALUES($1,9,$2,'active',1)").bind(owner).bind(metadata).execute(pool).await.unwrap();
    Fixture {
        reads,
        origin: endpoint.to_string().trim_end_matches('/').into(),
        github,
        _server: server,
    }
}
