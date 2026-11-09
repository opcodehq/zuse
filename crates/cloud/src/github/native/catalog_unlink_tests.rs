//! Catalogue I/O may finish after scoped unlink; missing authorization is never SQL NULL.
use super::*;
use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Fixture {
    gate: AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    repository_reads: AtomicUsize,
}
async fn repositories(State(fixture): State<Arc<Fixture>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers["authorization"], "Bearer fixture-user-token");
    fixture.repository_reads.fetch_add(1, Ordering::SeqCst);
    fixture.started.notify_one();
    if fixture.gate.load(Ordering::SeqCst) {
        fixture.release.notified().await;
    }
    Json(json!({"repositories":[
        {"full_name":"team/project","permissions":{"push":true},"private":true,"default_branch":"main"},
        {"full_name":"team/other","permissions":{"push":true},"private":true,"default_branch":"main"}
    ]}))
}
#[tokio::test]
#[ignore = "requires isolated localhost PostgreSQL"]
async fn catalog_unlink_during_repository_io_does_not_become_unrestricted_sql_null() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("github_catalog_unlink_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let scope = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
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
    let fixture = Arc::new(Fixture::default());
    let metadata = json!({"id":9,"account":{"id":81,"login":"team","type":"Organization"},"repository_selection":"all","suspended_at":null});
    let observed = metadata.clone();
    let app = Router::new()
        .route(
            "/app/installations/9",
            get(move || {
                let observed = observed.clone();
                async move { Json(observed) }
            }),
        )
        .route("/user/installations/9/repositories", get(repositories))
        .with_state(fixture.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        reqwest::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    use rsa::pkcs1::EncodeRsaPrivateKey;
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
    let access = Access {
        actor: crate::auth::Principal {
            account_id: "fixture-actor".into(),
        },
        owner_id: "fixture-actor".into(),
        scope: crate::access::Scope::Personal,
        membership: None,
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
    sqlx::query("INSERT INTO zuse_rust_github_users(actor_id,github_user_id,profile,sealed_credentials,state,epoch,version,updated_at) VALUES('fixture-actor',42,$1,$2,'connected',1,1,$3)").bind(json!({"id":42,"login":"fixture-actor"})).bind(&sealed).bind(now()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id) VALUES('fixture-actor')")
        .execute(&pool)
        .await
        .unwrap();
    // Cover both legitimate NULL and a selected subset. In each case the same
    // active row is removed by the production unlink path during HTTP I/O.
    for allowed in [None, Some(json!(["team/project"]))] {
        sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,allowed_repositories,state,updated_at) VALUES('fixture-actor',9,$1,$2,'active',$3) ON CONFLICT(owner_id,installation_id) DO UPDATE SET state='active',metadata=EXCLUDED.metadata,allowed_repositories=EXCLUDED.allowed_repositories")
            .bind(&metadata).bind(&allowed).bind(now()).execute(&pool).await.unwrap();
        fixture.gate.store(false, Ordering::SeqCst);
        let before = authority.catalog(&access).await.unwrap();
        assert_eq!(before.installations.len(), 1);
        assert_eq!(
            before.repositories.len(),
            if allowed.is_none() { 2 } else { 1 },
            "active SQL NULL and selected subsets retain their existing semantics"
        );
        // Drain the ungated observation's permit before waiting on the race.
        fixture.started.notified().await;
        fixture.gate.store(true, Ordering::SeqCst);
        let listing = authority.catalog(&access);
        let unlink = async {
            fixture.started.notified().await;
            let removed = tokio::time::timeout(
                Duration::from_secs(2),
                authority.remove_installation(&access, 9),
            )
            .await
            .expect("catalog must not hold SQL locks across repository HTTP")
            .unwrap();
            assert_eq!(removed["removed"], true);
            assert_eq!(removed["accountDisconnected"], false);
            fixture.release.notify_one();
        };
        let (after, ()) = tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(listing, unlink)
        })
        .await
        .unwrap();
        let after = after.unwrap();
        assert!(
            after.installations.is_empty(),
            "removed link must not reappear from an already accepted upstream reply"
        );
        assert!(
            after.repositories.is_empty(),
            "missing active row must never mean unrestricted SQL NULL"
        );
        let identity:(String,String,i64,bool)=sqlx::query_as("SELECT state,sealed_credentials,version,refresh_uncertain FROM zuse_rust_github_users WHERE actor_id='fixture-actor'").fetch_one(&pool).await.unwrap();
        assert_eq!(
            identity,
            ("connected".into(), sealed.clone(), 1, false),
            "scoped unlink never rotates or disconnects the actor's credentials"
        );
        let state:String=sqlx::query_scalar("SELECT state FROM zuse_rust_github_installations WHERE owner_id='fixture-actor' AND installation_id=9").fetch_one(&pool).await.unwrap();
        assert_eq!(state, "removed");
    }
    assert_eq!(fixture.repository_reads.load(Ordering::SeqCst), 4);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
