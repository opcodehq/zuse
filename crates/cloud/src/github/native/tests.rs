#[cfg(test)]
mod tests {
    use super::super::*;
    #[test]
    fn webhook_signature_retains_the_original_hard_payload_boundary() {
        let body = vec![b' '; GITHUB_WEBHOOK_BODY_LIMIT + 1];
        let mut mac = Hmac::<Sha256>::new_from_slice(b"webhook-secret").unwrap();
        mac.update(&body[..GITHUB_WEBHOOK_BODY_LIMIT]);
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(
            verify_signature(
                b"webhook-secret",
                &signature,
                &body[..GITHUB_WEBHOOK_BODY_LIMIT]
            )
            .is_ok()
        );
        assert!(verify_signature(b"webhook-secret", &signature, &body).is_err());
    }
    #[test]
    fn webhook_app_identity_preserves_optional_organization_and_ignored_event_contracts() {
        assert!(
            validate_webhook_installation(
                "123",
                "organization",
                &json!({"organization":{"id":81}})
            )
            .is_ok()
        );
        assert!(
            validate_webhook_installation(
                "123",
                "organization",
                &json!({"installation":{"id":9,"app_id":123}})
            )
            .is_ok()
        );
        assert!(
            validate_webhook_installation("123", "membership", &json!({"installation":{"id":9}}))
                .is_ok()
        );
        assert!(validate_webhook_installation("123", "ping", &json!({"zen":"benign"})).is_ok());
        assert!(
            validate_webhook_installation("123", "check_suite", &json!({"installation":{"id":9}}))
                .is_ok()
        );
        for value in [json!(null), json!("123"), json!(0), json!(-1), json!(999)] {
            assert!(
                validate_webhook_installation(
                    "123",
                    "member",
                    &json!({"installation":{"id":9,"app_id":value}})
                )
                .is_err()
            );
        }
    }
    #[test]
    fn scoped_credential_expiry_separates_provider_lifetime_from_authority_lease() {
        let at = 1_700_000_000_000i64;
        let body = |offset: i64| json!({"expires_at":chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at+offset).unwrap().to_rfc3339()});
        assert_eq!(
            scoped_token_expiry(&json!({}), None, at)
                .err()
                .unwrap()
                .code,
            "github_scoped_token_expiry_invalid"
        );
        assert_eq!(
            scoped_token_expiry(&json!({"expires_at":"malformed"}), None, at)
                .err()
                .unwrap()
                .code,
            "github_scoped_token_expiry_invalid"
        );
        assert_eq!(
            scoped_token_expiry(&body(-1), None, at).err().unwrap().code,
            "github_scoped_token_expired"
        );
        // Live provider contract: scoped child gets fresh eight hours although
        // the parent has only six and a half hours remaining.
        let expiry = scoped_token_expiry(&body(28_800_000), Some(at + 23_400_000), at).unwrap();
        assert_eq!(expiry.provider_expires_at, at + 28_800_000);
        assert_eq!(expiry.authority_expires_at, at + 3_600_000);
        assert_eq!(
            scoped_token_expiry(&body(28_800_000), Some(at + 900_000), at)
                .unwrap()
                .authority_expires_at,
            at + 900_000
        );
        assert_eq!(
            scoped_token_expiry(&body(600_000), None, at)
                .unwrap()
                .authority_expires_at,
            at + 600_000
        );
        assert_eq!(
            scoped_token_expiry(&body(28_800_000), Some(at), at)
                .err()
                .unwrap()
                .code,
            "github_reconnect_required"
        );
    }
    #[test]
    fn scoped_repository_intersection_never_uses_a_foreign_or_readonly_repository() {
        let installation = json!({"account":{"login":"team"}});
        let good = json!({"full_name":"team/project","permissions":{"push":true}});
        assert!(repository_allowed(&installation, None, &good));
        assert!(!repository_allowed(
            &installation,
            Some(&json!(["team/other"])),
            &good
        ));
        assert!(!repository_allowed(
            &installation,
            None,
            &json!({"full_name":"foreign/project","permissions":{"push":true}})
        ));
        assert!(!repository_allowed(
            &installation,
            None,
            &json!({"full_name":"team/project","permissions":{"pull":true}})
        ));
        assert!(!repository_allowed(
            &installation,
            None,
            &json!({"full_name":"team/../project","permissions":{"push":true}})
        ));
        assert!(!repository_allowed(&installation, Some(&json!({})), &good));
    }
    #[test]
    fn nonexpiring_and_rotating_credentials_have_distinct_validity() {
        let parsed = parse_credentials(&json!({"access_token":"access"}), 1000).unwrap();
        assert_eq!(parsed.expires_at, None);
        assert!(
            parse_credentials(
                &json!({"access_token":"access","refresh_token":"renew"}),
                1000
            )
            .is_err()
        );
        for expiry in [json!(0), json!(-1), json!(1.5), json!("3600"), Value::Null] {
            assert!(
                parse_credentials(&json!({"access_token":"access","expires_in":expiry}), 1000)
                    .is_err()
            );
        }
        let parsed=parse_credentials(&json!({"access_token":"access","expires_in":3600,"refresh_token":"renew","refresh_token_expires_in":10000}),1000).unwrap();
        assert_eq!(parsed.expires_at, Some(3_601_000));
        assert_eq!(parsed.refresh_expires_at, Some(10_001_000));
    }
    #[test]
    fn identity_uses_verified_numeric_id_and_safe_commit_email() {
        let user = public_user(&json!({"id":12,"login":"actor","name":"<script>"})).unwrap();
        assert_eq!(user.email, "12+actor@users.noreply.github.com");
        assert_eq!(user.name, "actor");
        assert!(public_user(&json!({"id":0,"login":"actor"})).is_err());
        assert!(public_user(&json!({"id":12,"login":"actor\nforged"})).is_err());
    }
    #[test]
    fn webhook_requires_raw_body_signature_and_private_secret() {
        let body = b"{\"action\":\"deleted\"}";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(body);
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(verify_signature(b"secret", &signature, body).is_ok());
        assert!(verify_signature(b"secret", &signature, b"{}").is_err());
        assert!(verify_signature(b"", &signature, body).is_err());
        assert!(verify_signature(b"secret", "sha256=nope", body).is_err());
    }
}

#[cfg(test)]
mod database {
    use super::super::*;
    use axum::{
        Json, Router,
        extract::{Form, Path, State},
        http::{HeaderMap, StatusCode},
        routing::{delete, get, post},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[derive(Default)]
    struct Fixture {
        refreshes: AtomicUsize,
        installation_reads: AtomicUsize,
        membership_reads: AtomicUsize,
        repository_tokens: std::sync::Mutex<Vec<String>>,
        scoped_tokens: std::sync::Mutex<Vec<String>>,
        revoked_tokens: std::sync::Mutex<Vec<String>>,
        fail_repositories: AtomicBool,
        fail_revocation: AtomicBool,
        fail_installation: AtomicBool,
        pull_request_drafts: std::sync::Mutex<Vec<bool>>,
        outside_collaborator: AtomicBool,
        departed_member: AtomicBool,
        read_only_repository: AtomicBool,
        overlap_scoped: AtomicBool,
        scope_overlap_ready: tokio::sync::Notify,
        gate_scoped: AtomicBool,
        scoped_started: tokio::sync::Notify,
        scoped_release: tokio::sync::Notify,
    }
    async fn user(headers: HeaderMap) -> Json<Value> {
        assert!(headers.get("authorization").is_some());
        if headers["authorization"] == "Bearer member-two-access" {
            Json(json!({"id":43,"login":"member-two","name":"Second Member"}))
        } else {
            Json(json!({"id":42,"login":"member","name":"Member"}))
        }
    }
    async fn token(
        State(state): State<Arc<Fixture>>,
        Form(body): Form<std::collections::HashMap<String, String>>,
    ) -> Json<Value> {
        if body
            .get("code")
            .is_some_and(|code| code == "member-two-code")
        {
            return Json(
                json!({"access_token":"member-two-access","expires_in":28800,"refresh_token":"member-two-refresh","refresh_token_expires_in":100000}),
            );
        }
        state.refreshes.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(80)).await;
        Json(
            json!({"access_token":"rotated-access","expires_in":28800,"refresh_token":"rotated-refresh","refresh_token_expires_in":100000}),
        )
    }
    async fn installation(
        State(state): State<Arc<Fixture>>,
        Path(id): Path<i64>,
    ) -> (StatusCode, Json<Value>) {
        state.installation_reads.fetch_add(1, Ordering::SeqCst);
        if state.overlap_scoped.load(Ordering::SeqCst) {
            tokio::time::timeout(Duration::from_secs(1), state.scope_overlap_ready.notified()).await.expect("token scoping must proceed while installation verification is blocked");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        if state.fail_installation.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
        }
        (
            StatusCode::OK,
            Json(
                json!({"id":id,"account":{"id":81,"login":"team","type":"Organization"},"repository_selection":"all","suspended_at":null}),
            ),
        )
    }
    async fn repositories(
        State(state): State<Arc<Fixture>>,
        headers: HeaderMap,
    ) -> (StatusCode, Json<Value>) {
        assert_ne!(headers["authorization"], "Bearer installation-admin-token");
        state
            .repository_tokens
            .lock()
            .unwrap()
            .push(headers["authorization"].to_str().unwrap().to_owned());
        if state.fail_repositories.load(Ordering::SeqCst) {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
        }
        (
            StatusCode::OK,
            Json(
                json!({"repositories":[{"full_name":"team/project","permissions":{"push":!state.read_only_repository.load(Ordering::SeqCst)},"private":true,"default_branch":"main"}]}),
            ),
        )
    }
    async fn revoke(State(state): State<Arc<Fixture>>, Json(value): Json<Value>) -> StatusCode {
        let token = value["access_token"].as_str().unwrap();
        assert!(matches!(token, "rotated-access" | "member-two-access"));
        state.revoked_tokens.lock().unwrap().push(token.to_owned());
        if state.fail_revocation.load(Ordering::SeqCst) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NO_CONTENT
        }
    }
    async fn scoped(State(state): State<Arc<Fixture>>, Json(value): Json<Value>) -> Json<Value> {
        if state.overlap_scoped.load(Ordering::SeqCst) { state.scope_overlap_ready.notify_one(); }
        if state.gate_scoped.load(Ordering::SeqCst) {
            state.scoped_started.notify_one();
            state.scoped_release.notified().await;
        }
        let token = value["access_token"].as_str().unwrap();
        assert!(matches!(token, "rotated-access" | "member-two-access"));
        state.scoped_tokens.lock().unwrap().push(token.to_owned());
        assert_eq!(value["target"], "team");
        assert_eq!(value["repositories"], json!(["project"]));
        Json(
            json!({"token":if token=="member-two-access" {"member-two-scoped"} else {"repo-scoped-access"},"expires_at":chrono::Utc::now().checked_add_signed(chrono::Duration::minutes(30)).unwrap().to_rfc3339()}),
        )
    }
    async fn pull_request(
        State(state): State<Arc<Fixture>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        assert_eq!(
            headers.get("authorization").unwrap(),
            "Bearer repo-scoped-access"
        );
        assert_eq!(body["head"], "test/proof");
        assert_eq!(body["base"], "main");
        assert_eq!(body["title"], "Fixture proof");
        let draft = body["draft"].as_bool().expect("explicit outbound bool");
        state.pull_request_drafts.lock().unwrap().push(draft);
        (
            StatusCode::CREATED,
            Json(
                json!({"html_url":"https://github.com/team/project/pull/17","number":17,"draft":draft}),
            ),
        )
    }
    struct BrowserIdentity;
    #[async_trait::async_trait]
    impl crate::auth::Identity for BrowserIdentity {
        async fn verify(&self, _: &str) -> Result<crate::auth::Principal, ApiError> {
            Err(ApiError::unauthorized("test_bearer_unused"))
        }
        async fn membership(
            &self,
            actor: &str,
            org: &str,
        ) -> Result<crate::auth::Membership, ApiError> {
            assert!(matches!(actor, "actor-one" | "actor-two"));
            assert_eq!(org, "team");
            Ok(crate::auth::Membership {
                id: format!("membership-{actor}"),
                user_id: actor.into(),
                organization_id: org.into(),
                status: "active".into(),
                role: crate::auth::MembershipRole {
                    slug: if actor == "actor-two" {
                        "admin"
                    } else {
                        "member"
                    }
                    .into(),
                },
            })
        }
    }
    include!("departure_tests.rs");
    fn access(actor: &str) -> Access {
        Access {
            actor: crate::auth::Principal {
                account_id: actor.into(),
            },
            owner_id: "organization:team".into(),
            scope: crate::access::Scope::Organization {
                organization_id: "team".into(),
            },
            membership: Some(crate::auth::Membership {
                id: format!("membership-{actor}"),
                user_id: actor.into(),
                organization_id: "team".into(),
                status: "active".into(),
                role: crate::auth::MembershipRole {
                    slug: "member".into(),
                },
            }),
        }
    }
    async fn webhook_app_identity_contract(
        app: &Router,
        authority: &Arc<NativeGithub>,
        pool: &PgPool,
        fixture: &Fixture,
    ) {
        async fn deliver(
            app: &Router,
            event: &str,
            value: Value,
            secret: &[u8],
            id: &str,
        ) -> StatusCode {
            use tower::ServiceExt;
            let body = serde_json::to_vec(&value).unwrap();
            let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
            mac.update(&body);
            let signature = format!(
                "sha256={}",
                mac.finalize()
                    .into_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            app.clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/v1/cloud/github/webhook")
                        .header("x-hub-signature-256", signature)
                        .header("x-github-delivery", id)
                        .header("x-github-event", event)
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }
        let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries")
            .fetch_one(pool)
            .await
            .unwrap();
        let epoch = authority.membership_revision();
        let upstream = fixture.installation_reads.load(Ordering::SeqCst);
        for (index, (event, value, valid_signature, expected)) in [
            (
                "installation",
                json!({"installation":{"id":9,"app_id":999}}),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "installation",
                json!({"installation":{"id":9}}),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "installation_repositories",
                json!({"installation":{"id":9,"app_id":"123"}}),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "installation_repositories",
                json!({"installation":{"id":0,"app_id":123}}),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "organization",
                json!({"installation":{"id":9,"app_id":999},"organization":{"id":81}}),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "installation",
                json!({"installation":{"id":9,"app_id":123}}),
                false,
                StatusCode::UNAUTHORIZED,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let status = deliver(
                app,
                event,
                value,
                if valid_signature {
                    b"webhook-secret"
                } else {
                    b"wrong-secret"
                },
                &format!("identity-invalid-{index}"),
            )
            .await;
            assert_eq!(
                status, expected,
                "invalid App identity/signature must fail before webhook effects, case {index}"
            );
            assert_eq!(
                authority.membership_revision(),
                epoch,
                "invalid identity must not invalidate authorization"
            );
            assert_eq!(
                fixture.installation_reads.load(Ordering::SeqCst),
                upstream,
                "invalid identity must not call GitHub"
            );
            let actual: i64 =
                sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries")
                    .fetch_one(pool)
                    .await
                    .unwrap();
            assert_eq!(
                actual, receipts,
                "invalid identity must not persist a receipt"
            );
        }
        let value = json!({"action":"created","installation":{"id":9,"app_id":123}});
        for _ in 0..2 {
            assert_eq!(
                deliver(
                    app,
                    "installation",
                    value.clone(),
                    b"webhook-secret",
                    "identity-valid-once"
                )
                .await,
                StatusCode::NO_CONTENT
            );
        }
        let accepted:i64=sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries WHERE delivery_id='identity-valid-once' AND completed").fetch_one(pool).await.unwrap();
        assert_eq!(
            accepted, 1,
            "valid matching App event must remain durably idempotent"
        );
        assert_eq!(
            deliver(
                app,
                "installation",
                json!({"action":"deleted","installation":{"id":9,"app_id":123}}),
                b"webhook-secret",
                "identity-valid-once"
            )
            .await,
            StatusCode::CONFLICT,
            "changed body cannot reuse a delivery ID"
        );
    }

    async fn webhook_payload_contract(app: &Router, pool: &PgPool) {
        use tower::ServiceExt;
        fn signature(body: &[u8], secret: &[u8]) -> String {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
            mac.update(body);
            format!(
                "sha256={}",
                mac.finalize()
                    .into_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        }
        fn request(
            body: axum::body::Body,
            signature: String,
            id: &str,
        ) -> axum::http::Request<axum::body::Body> {
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/cloud/github/webhook")
                .header("x-hub-signature-256", signature)
                .header("x-github-delivery", id)
                .header("x-github-event", "ping")
                .body(body)
                .unwrap()
        }
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries")
            .fetch_one(pool)
            .await
            .unwrap();
        let large = serde_json::to_vec(
            &json!({"zen":"bounded synthetic fixture","padding":"x".repeat(1024*1024+1)}),
        )
        .unwrap();
        assert!(large.len() > 1024 * 1024 && large.len() < 25 * 1024 * 1024);
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(request(
                    axum::body::Body::from(large.clone()),
                    signature(&large, b"webhook-secret"),
                    "payload-large-valid",
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::NO_CONTENT,
                "valid GitHub payload above1MiB must reach the signed receiver"
            );
        }
        let mut maximum = vec![b' '; 25 * 1024 * 1024];
        maximum[..2].copy_from_slice(b"{}");
        let response = app
            .clone()
            .oneshot(request(
                axum::body::Body::from(maximum.clone()),
                signature(&maximum, b"webhook-secret"),
                "payload-exact-maximum",
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "the exact original25MiB boundary remains valid"
        );
        drop(maximum);
        let response = app
            .clone()
            .oneshot(request(
                axum::body::Body::from(large.clone()),
                signature(&large, b"wrong-secret"),
                "payload-large-bad-signature",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let oversized = vec![b' '; 25 * 1024 * 1024 + 1];
        let response = app
            .clone()
            .oneshot(request(
                axum::body::Body::from(oversized.clone()),
                signature(&oversized, b"webhook-secret"),
                "payload-oversize",
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "the original25MiB hard bound remains"
        );
        drop(oversized);
        let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(
            actual,
            before + 2,
            "invalid signature and oversize cannot create receipts; large duplicate remains one row"
        );
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let first_body = axum::body::Body::from_stream(futures::stream::once({
            let started = started.clone();
            let release = release.clone();
            async move {
                started.notify_one();
                release.notified().await;
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
            }
        }));
        let first = tokio::spawn(app.clone().oneshot(request(
            first_body,
            signature(b"{}", b"webhook-secret"),
            "payload-held",
        )));
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        let polled = Arc::new(AtomicUsize::new(0));
        let second_body = axum::body::Body::from_stream(futures::stream::once({
            let polled = polled.clone();
            async move {
                polled.fetch_add(1, Ordering::SeqCst);
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"{}"))
            }
        }));
        let second = app
            .clone()
            .oneshot(request(
                second_body,
                signature(b"{}", b"webhook-secret"),
                "payload-busy",
            ))
            .await
            .unwrap();
        assert_eq!(
            second.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "busy receiver explicitly rejects without a queue"
        );
        assert_eq!(
            polled.load(Ordering::SeqCst),
            0,
            "second request body must not be polled/buffered without the route permit"
        );
        let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM zuse_rust_github_deliveries")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(
            actual,
            before + 2,
            "neither held incomplete body nor busy request touches the database"
        );
        release.notify_one();
        assert_eq!(
            first.await.unwrap().unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let response = app
            .clone()
            .oneshot(request(
                axum::body::Body::from("{}"),
                signature(b"{}", b"webhook-secret"),
                "payload-after-release",
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "body/handler completion releases the permit"
        );
    }

    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn standalone_oauth_refresh_scoping_revocation_and_callback_fences() {
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("github_{}", uuid::Uuid::new_v4().simple());
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
        sqlx::raw_sql(include_str!("../../../migrations/0015_github.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let fixture = Arc::new(Fixture::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/user", get(user))
            .route("/login/oauth/access_token", post(token))
            .route("/app/installations/{id}", get(installation))
            .route("/user/installations",get(||async{Json(json!({"installations":[{"id":9,"account":{"id":81,"login":"team","type":"Organization"},"repository_selection":"all","suspended_at":null}]}))}))
            .route("/user/installations/{id}/repositories", get(repositories))
            .route(
                "/user/memberships/orgs/team",
                get(|State(state):State<Arc<Fixture>>,headers:HeaderMap| async move {
                    state.membership_reads.fetch_add(1,Ordering::SeqCst);
                    if state.outside_collaborator.load(Ordering::SeqCst) || (state.departed_member.load(Ordering::SeqCst) && headers["authorization"]=="Bearer rotated-access") {(StatusCode::NOT_FOUND,Json(json!({})))}
                    else {(StatusCode::OK,Json(json!({"state":"active","role":"member"})))}
                }),
            )
            .route("/applications/client/grant", delete(revoke))
            .route("/applications/client/token/scoped", post(scoped))
            .route("/repos/team/project/pulls",post(pull_request))
            .with_state(fixture.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        use rsa::pkcs1::EncodeRsaPrivateKey;
        let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let der = key.to_pkcs1_der().unwrap();
        let origin = reqwest::Url::parse(&format!("http://{address}/")).unwrap();
        let mut authority = Arc::new(NativeGithub {
            membership_epoch: std::sync::atomic::AtomicU64::new(0),
            observation_gates: Default::default(),
            fast_start_accounts: BTreeSet::from(["organization:team".into(), "actor-one".into()]),
            pool: pool.clone(),
            master: MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
            accounts: BTreeSet::from([
                "organization:team".into(),
                "organization:other-team".into(),
                "actor-one".into(),
            ]),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            config: Config {
                origin: reqwest::Url::parse("https://api.example.test/").unwrap(),
                app_id: "123".into(),
                slug: "fixture".into(),
                client_id: "client".into(),
                client_secret: Zeroizing::new("client-secret".into()),
                webhook_secret: Zeroizing::new("webhook-secret".into()),
                signing_key: jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()),
                api_origin: origin.clone(),
                oauth_origin: origin,
            },
        });
        // Production presentation uses a read-only pool. Native OAuth and
        // joining must reuse the separate writable runtime-authority pool.
        let read_scope = schema.clone();
        let read_only = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |connection, _| {
                let scope = read_scope.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={scope}"))
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET default_transaction_read_only=on")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        assert!(
            sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id) VALUES('must-not-write')")
                .execute(&read_only)
                .await
                .is_err()
        );
        Arc::get_mut(&mut authority).unwrap().pool = read_only.clone();
        assert_eq!(
            authority.ready().await.unwrap_err().code,
            "github_native_requires_write_pool"
        );
        Arc::get_mut(&mut authority).unwrap().pool = pool.clone();
        authority.ready().await.unwrap();
        read_only.close().await;
        let access = access("actor-one");
        let begun = authority.begin(&access).await.unwrap();
        let id = begun["operationId"].as_str().unwrap().to_owned();
        let state = reqwest::Url::parse(begun["url"].as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .to_string();
        let (_, browser) = authority.browser_start(&state, None).await.unwrap();
        assert!(authority.browser_start(&state, None).await.is_err());
        assert!(
            authority
                .callback(
                    &Callback {
                        state: state.clone(),
                        code: Some("code".into()),
                        error: None
                    },
                    Some(&opaque())
                )
                .await
                .is_err()
        );
        authority
            .callback(
                &Callback {
                    state: state.clone(),
                    code: Some("code".into()),
                    error: None,
                },
                Some(&browser),
            )
            .await
            .unwrap();
        assert!(
            authority
                .callback(
                    &Callback {
                        state: state.clone(),
                        code: Some("code".into()),
                        error: None
                    },
                    Some(&browser)
                )
                .await
                .is_err()
        );
        assert!(
            authority
                .operation(&self::access("other-actor"), &id)
                .await
                .is_err()
        );
        let mut store = crate::store::Store::from_pool(pool.clone());
        store.github_native = Some(authority.clone());
        let app = crate::routes::router(
            crate::routes::AppState {
                store,
                identity: Arc::new(BrowserIdentity),
                organizations_enabled: true,
                organization_ids: Arc::new(vec!["team".into()]),
                runtime: None,
                gateway: Arc::new(crate::gateway::Hub::default()),
            },
            vec![],
        );
        webhook_app_identity_contract(&app, &authority, &pool, &fixture).await;
        webhook_payload_contract(&app, &pool).await;
        use tower::ServiceExt;
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/cloud/github/browser/select")
                    .header("origin", "https://attacker.example")
                    .header("cookie", format!("__Host-zuse-github={browser}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(format!(
                        "state={state}&installationId=9"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // App-side bearer alone cannot consent on another OAuth browser's behalf.
        assert_eq!(
            authority
                .select(
                    &access,
                    &Selection {
                        operation_id: id.clone(),
                        installation_id: 9
                    }
                )
                .await
                .unwrap_err()
                .code,
            "github_browser_confirmation_required"
        );
        let pending = authority.operation(&access, &id).await.unwrap();
        assert_eq!(pending["state"], "authorizing");
        assert!(pending.get("user").is_none());
        let html = authority
            .browser_page(&state, Some(&browser))
            .await
            .unwrap();
        assert!(html.contains("Use team"));
        assert!(!html.contains("rotated-access"));
        assert!(
            authority
                .confirm_browser(
                    &BrowserSelection {
                        state: state.clone(),
                        installation_id: 9
                    },
                    Some(&opaque())
                )
                .await
                .is_err()
        );
        assert!(
            authority
                .confirm_browser(
                    &BrowserSelection {
                        state: state.clone(),
                        installation_id: 9
                    },
                    None
                )
                .await
                .is_err()
        );
        assert!(!authority.browser_origin_matches("https://attacker.example"));
        assert!(authority.browser_origin_matches("https://api.example.test"));
        authority
            .confirm_browser(
                &BrowserSelection {
                    state: state.clone(),
                    installation_id: 9,
                },
                Some(&browser),
            )
            .await
            .unwrap();
        assert!(
            authority
                .confirm_browser(
                    &BrowserSelection {
                        state: state.clone(),
                        installation_id: 10
                    },
                    Some(&browser)
                )
                .await
                .is_err()
        );
        // Member may connect their own token only to an existing org installation.
        assert!(
            authority
                .select(
                    &access,
                    &Selection {
                        operation_id: id.clone(),
                        installation_id: 9
                    }
                )
                .await
                .is_err()
        );
        sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,state,updated_at) VALUES('organization:team',9,$1,'active',$2)").bind(json!({"id":9,"account":{"id":81,"login":"team","type":"Organization"}})).bind(now()).execute(&pool).await.unwrap();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/cloud/github/browser/select")
                    .header("origin", "https://api.example.test")
                    .header("cookie", format!("__Host-zuse-github={browser}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(format!(
                        "state={state}&installationId=9"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("form-action 'self'")
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        let html = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert!(
            std::str::from_utf8(&html)
                .unwrap()
                .contains("GitHub connected")
        );
        assert_eq!(
            authority.operation(&access, &id).await.unwrap()["state"],
            "complete"
        );
        // Force a renewable expiry, then concurrently ask from two workspaces.
        let expired = Credentials {
            access_token: "expired-access".into(),
            expires_at: Some(now() - 1),
            refresh_token: Some("old-refresh".into()),
            refresh_expires_at: Some(now() + 100000),
        };
        let sealed = authority.seal("github-user:actor-one:1", &expired).unwrap();
        sqlx::query(
            "UPDATE zuse_rust_github_users SET sealed_credentials=$1 WHERE actor_id='actor-one'",
        )
        .bind(sealed)
        .execute(&pool)
        .await
        .unwrap();
        fixture.refreshes.store(0, Ordering::SeqCst);
        let (a, b) = tokio::join!(
            authority.credentials("actor-one"),
            authority.credentials("actor-one")
        );
        assert_eq!(a.unwrap().access_token, "rotated-access");
        assert_eq!(b.unwrap().access_token, "rotated-access");
        assert_eq!(fixture.refreshes.load(Ordering::SeqCst), 1);
        fixture.fail_repositories.store(true, Ordering::SeqCst);
        assert!(authority.catalog(&access).await.is_err());
        assert_eq!(
            authority
                .credentials("actor-one")
                .await
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("rotated-refresh")
        );
        fixture.fail_repositories.store(false, Ordering::SeqCst);
        fixture.overlap_scoped.store(true, Ordering::SeqCst);
        let grant = authority
            .scoped_git(&access.owner_id, "actor-one", "team/project")
            .await
            .unwrap();
        fixture.overlap_scoped.store(false, Ordering::SeqCst);
        assert_eq!(grant.token.as_str(), "repo-scoped-access");
        for draft in [false, true] {
            let pr = authority
                .create_pull_request(
                    &access,
                    "team/project",
                    "Fixture proof",
                    "test/proof",
                    "main",
                    "Test fixture",
                    draft,
                )
                .await
                .unwrap();
            assert_eq!(pr["draft"], draft);
            assert_eq!(pr["number"], 17);
        }
        assert_eq!(
            *fixture.pull_request_drafts.lock().unwrap(),
            vec![false, true]
        );

        assert!(
            authority
                .scoped_git(&access.owner_id, "actor-one", "foreign/project")
                .await
                .is_err()
        );
        let catalog = authority.catalog(&access).await.unwrap();
        assert_eq!(catalog.repositories.len(), 1);
        assert_eq!(catalog.can_manage_installations, Some(false));
        // Two members share an installation, never its administrator's token.
        // Complete the second actor's own browser-bound OAuth exchange.
        let second_access = self::access("actor-two");
        let second = authority.begin(&second_access).await.unwrap();
        let second_state = reqwest::Url::parse(second["url"].as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .to_string();
        let (_, second_browser) = authority.browser_start(&second_state, None).await.unwrap();
        authority
            .callback(
                &Callback {
                    state: second_state.clone(),
                    code: Some("member-two-code".into()),
                    error: None,
                },
                Some(&second_browser),
            )
            .await
            .unwrap();
        authority
            .confirm_browser(
                &BrowserSelection {
                    state: second_state,
                    installation_id: 9,
                },
                Some(&second_browser),
            )
            .await
            .unwrap();
        authority
            .select(
                &second_access,
                &Selection {
                    operation_id: second["operationId"].as_str().unwrap().into(),
                    installation_id: 9,
                },
            )
            .await
            .unwrap();
        fixture.repository_tokens.lock().unwrap().clear();
        fixture.scoped_tokens.lock().unwrap().clear();
        for (member, expected_user, expected_token) in [
            (&access, "member", "repo-scoped-access"),
            (&second_access, "member-two", "member-two-scoped"),
        ] {
            assert_eq!(
                authority.catalog(member).await.unwrap().user.unwrap().login,
                expected_user
            );
            assert_eq!(
                authority
                    .scoped_git(&member.owner_id, &member.actor.account_id, "team/project")
                    .await
                    .unwrap()
                    .token
                    .as_str(),
                expected_token
            );
        }
        assert_eq!(
            *fixture.scoped_tokens.lock().unwrap(),
            vec!["rotated-access", "member-two-access"]
        );
        let observed = fixture.repository_tokens.lock().unwrap().clone();
        assert_eq!(
            observed,
            vec![
                "Bearer rotated-access",
                "Bearer rotated-access",
                "Bearer member-two-access",
                "Bearer member-two-access"
            ]
        );
        // Exercise actual runtime delivery while storage is only prepared, before
        // checkout/engine commit, and fence an account switch under the same actor.
        sqlx::raw_sql(include_str!("../../../migrations/0001_control_plane.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider_sandbox_id text,runtime_credential_hash text,state text,desired_state text,request_config jsonb);CREATE TABLE api_github_enrollments(organization_id text,account_id text,installation_id bigint,data jsonb);CREATE TABLE api_github_join_policies(organization_id text,installation_id bigint,data jsonb);").execute(&pool).await.unwrap();
        // A fresh OAuth identity cannot replace the identity of an existing
        // GitHub-managed member, and finalization fences policy changes.
        let enrollment = json!({"blocked":false,"githubUserId":42,"githubOrgId":81,"installationId":9,"revision":"enrollment-v1","memberId":"membership-actor-one"});
        let policy = json!({"githubOrgId":81,"installationId":9,"revision":"policy-v1"});
        sqlx::query("INSERT INTO api_github_enrollments VALUES('team','actor-one',9,$1)")
            .bind(&enrollment)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_github_join_policies VALUES('team',9,$1)")
            .bind(&policy)
            .execute(&pool)
            .await
            .unwrap();
        let mut pending_identity = Pending {
            credentials: authority.credentials("actor-one").await.unwrap(),
            profile: json!({"id":43,"login":"different"}),
            browser_installation: Some(9),
        };
        assert_eq!(
            authority
                .verify_pending_enrollment(&access, &pending_identity)
                .await
                .unwrap_err()
                .code,
            "github_enrollment_identity_mismatch"
        );
        pending_identity.profile = json!({"id":42,"login":"member"});
        let snapshot = authority
            .verify_pending_enrollment(&access, &pending_identity)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        authority
            .lock_enrollment(&mut tx, &access, &snapshot)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0018_github_joining_jobs.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE api_github_org_members(installation_id bigint,github_user_id bigint)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("ALTER TABLE api_github_join_policies ADD COLUMN github_org_id bigint")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE api_github_join_policies SET github_org_id=81")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_github_join_policies VALUES('off-cohort',9,$1,81)")
            .bind(&policy)
            .execute(&pool)
            .await
            .unwrap();
        // A signed membership event invalidates durable policy snapshots across
        // processes without trusting its membership claim or enrolling anyone.
        let body = serde_json::to_vec(&json!({"action":"member_removed","organization":{"id":81}}))
            .unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"webhook-secret").unwrap();
        mac.update(&body);
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        authority
            .webhook(&signature, "membership-durable-one", "organization", &body)
            .await
            .unwrap();
        let changed = authority
            .enrollment_snapshot(&access)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            changed["policy"]["revision"],
            snapshot.as_ref().unwrap()["policy"]["revision"]
        );
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(
            authority
                .lock_enrollment(&mut tx, &access, &snapshot)
                .await
                .unwrap_err()
                .code,
            "github_enrollment_changed"
        );
        tx.rollback().await.unwrap();
        authority
            .webhook(&signature, "membership-durable-one", "organization", &body)
            .await
            .unwrap();
        assert_eq!(
            authority
                .enrollment_snapshot(&access)
                .await
                .unwrap()
                .unwrap(),
            changed,
            "duplicate delivery must not invalidate again"
        );
        authority
            .webhook(&signature, "membership-durable-two", "organization", &body)
            .await
            .unwrap();
        assert_ne!(
            authority
                .enrollment_snapshot(&access)
                .await
                .unwrap()
                .unwrap()["policy"]["revision"],
            changed["policy"]["revision"]
        );
        let untouched: Value = sqlx::query_scalar(
            "SELECT data FROM api_github_join_policies WHERE organization_id='off-cohort'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(untouched, policy);
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_github_enrollments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "membership events must not auto-enroll");
        let versions: i64 = sqlx::query_scalar(
            "SELECT requested_version FROM zuse_rust_github_join_jobs WHERE installation_id=9",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(versions, 2);
        sqlx::query(
            "UPDATE api_github_join_policies SET data=jsonb_set(data,'{revision}','\"policy-v2\"')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(
            authority
                .lock_enrollment(&mut tx, &access, &snapshot)
                .await
                .unwrap_err()
                .code,
            "github_enrollment_changed"
        );
        tx.rollback().await.unwrap();
        sqlx::query("DELETE FROM api_github_enrollments")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM api_github_join_policies")
            .execute(&pool)
            .await
            .unwrap();
        use rsa::traits::PublicKeyParts;
        let public=json!({"kty":"RSA","e":URL_SAFE_NO_PAD.encode(key.e().to_bytes_be()),"n":URL_SAFE_NO_PAD.encode(key.n().to_bytes_be())}).to_string();
        let thumb = crate::crypto::credential_thumbprint(&public).unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([71;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let runtime = crate::runtime::RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
            crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
                .unwrap(),
        );
        runtime
            .configure_scope_authority(
                Arc::new(BrowserIdentity),
                Some(authority.clone()),
                vec!["team".into()],
            )
            .unwrap();
        let runtime_hash = Sha256::digest(b"runtime-fixture")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let config = json!({"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeGeneration":1,"gatewayEpoch":1,"runtimeBootstrapReceipt":{"workspaceId":"task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now()+600000,"runtimeCredentialKeyThumbprint":thumb,"runtimeSigningKeyThumbprint":"signing-thumb","rustActorId":"actor-one","gitAuthMode":"broker-v1","rustGithubUserId":42,"rustRepositoryCheckout":{"repositoryUrl":"https://github.com/team/project.git"},"rustRuntimeStorage":{"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"prepared","generation":1,"sourceSandboxId":"sandbox","storageIncarnationId":"disk","credentialKeyThumbprint":thumb,"signingKeyThumbprint":"signing-thumb"},"sharingPolicy":{"audience":"private","permission":"view","creatorSubject":"actor-one","creatorMembershipId":"membership-actor-one","grants":[],"revision":1}});
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','task','rust',1,$1)")
            .bind(now())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','organization:team','sandbox',$1,'starting','ready',$2)").bind(runtime_hash).bind(&config).execute(&pool).await.unwrap();
        let request = || crate::runtime::github::GitGrantRequest {
            protocol_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_generation: 1,
            credential_public_jwk: public.clone(),
        };
        let sealed = runtime
            .github_grant(&authority, "task", "runtime-fixture", request(), now())
            .await
            .unwrap();
        assert!(sealed["sealedGrant"].as_str().unwrap().split('.').count() == 5);
        assert!(!sealed.to_string().contains("rotated-access"));
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustGithubUserId}','43') WHERE workspace_id='task'").execute(&pool).await.unwrap();
        assert_eq!(
            runtime
                .github_grant(&authority, "task", "runtime-fixture", request(), now())
                .await
                .unwrap_err()
                .code,
            "github_account_changed"
        );
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustGithubUserId}','42') WHERE workspace_id='task'").execute(&pool).await.unwrap();
        assert_eq!(
            runtime
                .github_grant(&authority, "task", "wrong-runtime", request(), now())
                .await
                .unwrap_err()
                .code,
            "workspace_runtime_fenced"
        );
        departure_closes_gateway_and_rejects_queued_author(
            &pool, &authority, &fixture, &config, &app,
        )
        .await;
        // A shared installation event inspects every enabled existing owner,
        // never creates another link or enrollment, and deduplicates the delivery.
        sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id) VALUES('organization:other-team'),('organization:disabled-team')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,state,updated_at) SELECT owner_id,9,$1,'active',$2 FROM zuse_rust_github_scopes WHERE owner_id IN ('organization:other-team','organization:disabled-team')").bind(json!({"id":9,"account":{"id":81,"login":"team","type":"Organization"}})).bind(now()).execute(&pool).await.unwrap();
        let before:Vec<(String,i64)>=sqlx::query_as("SELECT owner_id,revision FROM zuse_rust_github_installations WHERE installation_id=9 ORDER BY owner_id").fetch_all(&pool).await.unwrap();
        fixture.installation_reads.store(0, Ordering::SeqCst);
        // Webhook payload order is not authoritative; refetch current GitHub state.
        let payload =
            serde_json::to_vec(&json!({"action":"deleted","installation":{"id":9,"app_id":123}}))
                .unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"webhook-secret").unwrap();
        mac.update(&payload);
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        authority
            .webhook(&signature, "delivery-one", "installation", &payload)
            .await
            .unwrap();
        authority
            .webhook(&signature, "delivery-one", "installation", &payload)
            .await
            .unwrap();
        let after:Vec<(String,i64)>=sqlx::query_as("SELECT owner_id,revision FROM zuse_rust_github_installations WHERE installation_id=9 ORDER BY owner_id").fetch_all(&pool).await.unwrap();
        assert_eq!(
            fixture.installation_reads.load(Ordering::SeqCst),
            2,
            "one current upstream inspection per enabled linked owner, no duplicate replay"
        );
        assert_eq!(
            before.len(),
            after.len(),
            "webhooks never create installation links"
        );
        for ((owner, old), (same, new)) in before.iter().zip(&after) {
            assert_eq!(owner, same);
            assert_eq!(
                *new,
                *old + if owner == "organization:disabled-team" {
                    0
                } else {
                    1
                }
            );
        }
        let enrollment_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM api_github_enrollments")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(enrollment_count, 0);
        sqlx::query("DELETE FROM zuse_rust_github_installations WHERE owner_id IN ('organization:other-team','organization:disabled-team')").execute(&pool).await.unwrap();
        let state: String = sqlx::query_scalar(
            "SELECT state FROM zuse_rust_github_installations WHERE installation_id=9",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "active");
        fixture.fail_installation.store(true, Ordering::SeqCst);
        assert!(authority.observe(&access.owner_id, 9).await.is_err());
        let state: String = sqlx::query_scalar(
            "SELECT state FROM zuse_rust_github_installations WHERE installation_id=9",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "active");
        fixture.fail_installation.store(false, Ordering::SeqCst);
        let stale = authority.observe(&access.owner_id, 9);
        let supersede = async {
            tokio::time::sleep(Duration::from_millis(15)).await;
            sqlx::query("UPDATE zuse_rust_github_installations SET observation_id='newer-observation' WHERE installation_id=9").execute(&pool).await.unwrap();
        };
        let (old, _) = tokio::join!(stale, supersede);
        assert_eq!(old.unwrap_err().code, "github_observation_superseded");
        // Personal collaborators may select only their own writable subset;
        // organization enrollment still requires actual membership.
        let mut personal = self::access("actor-one");
        personal.owner_id = "actor-one".into();
        personal.scope = crate::access::Scope::Personal;
        personal.membership = None;
        let metadata = json!({"id":9,"account":{"id":81,"login":"team","type":"Organization"}});
        fixture.outside_collaborator.store(true, Ordering::SeqCst);
        assert_eq!(
            authority
                .installation_access(&personal, &metadata, &json!({"id":42}), "rotated-access")
                .await
                .unwrap(),
            Some(json!(["team/project"]))
        );
        assert_eq!(
            authority
                .installation_access(&access, &metadata, &json!({"id":42}), "rotated-access")
                .await
                .unwrap_err()
                .code,
            "github_organization_membership_required"
        );
        fixture.read_only_repository.store(true, Ordering::SeqCst);
        assert_eq!(
            authority
                .installation_access(&personal, &metadata, &json!({"id":42}), "rotated-access")
                .await
                .unwrap_err()
                .code,
            "github_repository_access_denied"
        );
        fixture.read_only_repository.store(false, Ordering::SeqCst);
        fixture.outside_collaborator.store(false, Ordering::SeqCst);
        sqlx::query("INSERT INTO zuse_rust_github_scopes(owner_id,epoch) VALUES('actor-one',1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO zuse_rust_github_installations(owner_id,installation_id,metadata,state,allowed_repositories,updated_at) VALUES('actor-one',9,$1,'active',$2,$3)").bind(&metadata).bind(json!(["team/project"])).bind(now()).execute(&pool).await.unwrap();
        assert_eq!(
            authority
                .remove_installation(&access, 9)
                .await
                .unwrap_err()
                .code,
            "github_installation_admin_required"
        );
        // A token mint already in flight cannot finish after scoped unlink.
        fixture.gate_scoped.store(true, Ordering::SeqCst);
        let grant = authority.scoped_git("actor-one", "actor-one", "team/project");
        let unlink = async {
            fixture.scoped_started.notified().await;
            let removed = authority.remove_installation(&personal, 9).await.unwrap();
            assert_eq!(removed["accountDisconnected"], false);
            fixture.scoped_release.notify_one();
        };
        let (grant, _) = tokio::join!(grant, unlink);
        // Parallel scoping can observe unlink either during the App inspection
        // or at the final epoch fence. Both must reject delivery of the token.
        assert!(matches!(
            grant.err().expect("unlinked grant must fail").code,
            "github_installation_changed" | "github_observation_superseded"
        ));
        fixture.gate_scoped.store(false, Ordering::SeqCst);
        assert_eq!(
            authority.remove_installation(&personal, 9).await.unwrap()["changed"],
            false
        );
        assert!(authority.credentials("actor-one").await.is_ok());
        let org_active:bool=sqlx::query_scalar("SELECT state='active' FROM zuse_rust_github_installations WHERE owner_id='organization:team' AND installation_id=9").fetch_one(&pool).await.unwrap();
        assert!(
            org_active,
            "personal unlink must preserve organization authorization"
        );
        assert!(
            authority
                .scoped_git("actor-one", "actor-one", "team/project")
                .await
                .is_err()
        );
        let mut tx = pool.begin().await.unwrap();
        assert_eq!(
            authority
                .lock_scope_epoch(&mut tx, "actor-one", 1)
                .await
                .unwrap_err()
                .code,
            "github_operation_unavailable"
        );
        tx.rollback().await.unwrap();
        fixture.fail_revocation.store(true, Ordering::SeqCst);
        assert!(authority.disconnect(&access).await.is_err());
        assert!(authority.credentials("actor-one").await.is_err());
        let state: String = sqlx::query_scalar(
            "SELECT state FROM zuse_rust_github_users WHERE actor_id='actor-one'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "revoking");
        fixture.fail_revocation.store(false, Ordering::SeqCst);
        authority.disconnect(&access).await.unwrap();
        let cleared:bool=sqlx::query_scalar("SELECT sealed_credentials IS NULL FROM zuse_rust_github_users WHERE actor_id='actor-one'").fetch_one(&pool).await.unwrap();
        assert!(cleared);
        assert_eq!(
            fixture
                .revoked_tokens
                .lock()
                .unwrap()
                .iter()
                .filter(|token| token.as_str() == "member-two-access")
                .count(),
            0
        );
        assert_eq!(
            authority
                .catalog(&second_access)
                .await
                .unwrap()
                .user
                .unwrap()
                .login,
            "member-two"
        );
        assert_eq!(
            authority
                .scoped_git(&second_access.owner_id, "actor-two", "team/project")
                .await
                .unwrap()
                .token
                .as_str(),
            "member-two-scoped"
        );
        assert_eq!(
            authority
                .credentials("actor-two")
                .await
                .unwrap()
                .access_token,
            "member-two-access"
        );
        assert!(
            authority.credentials("actor-one").await.is_err(),
            "a disconnected member cannot borrow the other member's identity"
        );
        authority.disconnect(&second_access).await.unwrap();
        // Cancelled browser transaction cannot later consume an OAuth code.
        let begun = authority.begin(&access).await.unwrap();
        authority
            .cancel(&access, begun["operationId"].as_str().unwrap())
            .await
            .unwrap();
        let state = reqwest::Url::parse(begun["url"].as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .to_string();
        assert!(authority.browser_start(&state, None).await.is_err());
        // A browser may save a new verified identity while an organization App
        // approval is pending. No installation is inferred from that consent.
        sqlx::raw_sql("CREATE TABLE api_github_identities(account_id text PRIMARY KEY,github_user_id bigint UNIQUE NOT NULL,data jsonb NOT NULL)").execute(&pool).await.unwrap();
        let begun = authority.begin(&access).await.unwrap();
        let identity_id = begun["operationId"].as_str().unwrap();
        let state = reqwest::Url::parse(begun["url"].as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .to_string();
        let (authorize_url, identity_browser) =
            authority.browser_start(&state, None).await.unwrap();
        assert!(authorize_url.contains("/login/oauth/authorize"));
        authority
            .callback(
                &Callback {
                    state: state.clone(),
                    code: Some("identity-only-code".into()),
                    error: None,
                },
                Some(&identity_browser),
            )
            .await
            .unwrap();
        assert_eq!(
            authority
                .finalize_identity(&access, identity_id)
                .await
                .unwrap_err()
                .code,
            "github_browser_confirmation_required"
        );
        assert!(
            authority
                .confirm_identity_browser(&state, Some(&opaque()))
                .await
                .is_err()
        );
        let forbidden = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/cloud/github/browser/identity")
                    .header("origin", "https://attacker.example")
                    .header("cookie", format!("__Host-zuse-github={identity_browser}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(format!("state={state}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        let accepted = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/cloud/github/browser/identity")
                    .header("origin", "https://api.example.test")
                    .header("cookie", format!("__Host-zuse-github={identity_browser}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(format!("state={state}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(
            authority.operation(&access, identity_id).await.unwrap()["state"],
            "complete"
        );
        let linked: i64 = sqlx::query_scalar(
            "SELECT github_user_id FROM api_github_identities WHERE account_id='actor-one'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, 42);
        // Reopening after approval reuses this authority identity, rather than
        // starting OAuth or choosing a different GitHub account again.
        let reopened = authority.begin(&access).await.unwrap();
        let state = reqwest::Url::parse(reopened["url"].as_str().unwrap())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .to_string();
        let (url, browser) = authority.browser_start(&state, None).await.unwrap();
        assert_eq!(
            reqwest::Url::parse(&url).unwrap().path(),
            "/v1/cloud/github/browser"
        );
        let expiry_before: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM zuse_rust_github_operations WHERE state_hash=$1",
        )
        .bind(hash(&state))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(authority.browser_start(&state, None).await.is_err());
        assert!(
            authority
                .browser_start(&state, Some(&opaque()))
                .await
                .is_err()
        );
        let (resumed, same_browser) = authority
            .browser_start(&state, Some(&browser))
            .await
            .unwrap();
        assert_eq!(resumed, url);
        assert_eq!(same_browser, browser);
        let expiry_after: i64 = sqlx::query_scalar(
            "SELECT expires_at FROM zuse_rust_github_operations WHERE state_hash=$1",
        )
        .bind(hash(&state))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(expiry_before, expiry_after);
        server.abort();
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}
