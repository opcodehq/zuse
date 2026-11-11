use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use hmac::{Hmac, Mac};
use tower::ServiceExt;
const MENTION: &str = include_str!("../../tests/fixtures/slack/mention.json");
const DM: &str = include_str!("../../tests/fixtures/slack/dm.json");
const THREAD: &str = include_str!("../../tests/fixtures/slack/thread.json");
fn signed(body: &[u8], timestamp: i64) -> HeaderMap {
    let mut mac = Hmac::<Sha256>::new_from_slice(b"signing-fixture").unwrap();
    mac.update(format!("v0:{timestamp}:").as_bytes());
    mac.update(body);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-slack-request-timestamp",
        timestamp.to_string().parse().unwrap(),
    );
    headers.insert(
        "x-slack-signature",
        format!("v0={:x}", mac.finalize().into_bytes())
            .parse()
            .unwrap(),
    );
    headers
}
#[test]
fn signatures_use_recorded_raw_bytes_and_five_minute_window() {
    for body in [MENTION, DM, THREAD] {
        let headers = signed(body.as_bytes(), 1793296800);
        assert!(http::verify("signing-fixture", &headers, body.as_bytes(), 1793297100).is_ok());
        assert!(http::verify("signing-fixture", &headers, body.as_bytes(), 1793297101).is_err());
        assert!(http::verify("signing-fixture", &headers, b"{}", 1793296800).is_err());
        assert!(http::verify("wrong-key", &headers, body.as_bytes(), 1793296800).is_err());
    }
}
#[test]
fn malformed_signatures_fail_without_panics() {
    for signature in [
        "v0=",
        "v1=abc",
        &format!("v0={}", "é".repeat(32)),
        &format!("v0={}", "z".repeat(64)),
    ] {
        let mut headers = signed(b"{}", 42);
        headers.insert("x-slack-signature", signature.parse().unwrap());
        assert!(http::verify("signing-fixture", &headers, b"{}", 42).is_err());
    }
}
#[test]
fn mentions_dms_and_explicit_followups_match_upstream() {
    let mention = http::event(&serde_json::from_str(MENTION).unwrap(), "U_ZUSE").unwrap();
    assert_eq!(mention.thread, mention.ts);
    let dm = http::event(&serde_json::from_str(DM).unwrap(), "U_ZUSE").unwrap();
    assert_eq!(dm.channel, "D_ADA");
    let thread = http::event(&serde_json::from_str(THREAD).unwrap(), "U_ZUSE").unwrap();
    assert_eq!(thread.thread, mention.ts);
    let mut value: Value = serde_json::from_str(DM).unwrap();
    value["event"]["thread_ts"] = json!(mention.ts);
    assert!(http::event(&value, "U_ZUSE").is_none());
    value["event"]["text"] = json!("<@U_ZUSE> Continue this task");
    assert!(http::event(&value, "U_ZUSE").is_some());
    value["event"]["type"] = json!("app_mention");
    value["event"]["bot_id"] = json!("B_BOT");
    assert!(http::event(&value, "U_ZUSE").is_none());
    value["event"].as_object_mut().unwrap().remove("bot_id");
    value["event"]["subtype"] = json!("message_changed");
    assert!(http::event(&value, "U_ZUSE").is_none());
}

#[derive(Clone)]
struct Identity;
#[async_trait::async_trait]
impl crate::auth::Identity for Identity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if token != "bob" {
            return Err(ApiError::unauthorized("invalid_token"));
        }
        Ok(Principal {
            account_id: "user-bob".into(),
        })
    }
    async fn membership(
        &self,
        _account: &str,
        _org: &str,
    ) -> Result<crate::auth::Membership, ApiError> {
        Err(ApiError::forbidden("membership_revoked"))
    }
}
async fn request(app: &Router, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header("authorization", "Bearer bob");
    let data = if let Some(body) = body {
        request = request
            .method("POST")
            .header("content-type", "application/json");
        body.to_string()
    } else {
        String::new()
    };
    let response = app
        .clone()
        .oneshot(request.body(Body::from(data)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn fixture() -> (
    PgPool,
    PgPool,
    String,
    Slack,
    Arc<std::sync::Mutex<Vec<(String, Value)>>>,
) {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL")
        .expect("explicit localhost PostgreSQL URL required");
    assert!(
        reqwest::Url::parse(&url)
            .unwrap()
            .host_str()
            .is_some_and(|h| h == "127.0.0.1" || h == "localhost")
    );
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("slack_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = format!("SET search_path TO {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _| {
            let search = search.clone();
            Box::pin(async move {
                sqlx::query(&search).execute(&mut *connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for sql in [
        include_str!("../../tests/fixtures/legacy-catalog.sql"),
        crate::jobs::MIGRATION,
        crate::runtime::mailbox::MIGRATION,
        crate::runtime::allocation::MIGRATION,
        include_str!("../../tests/fixtures/creation.sql"),
        MIGRATION,
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql("ALTER TABLE api_cloud_projects ADD COLUMN repository_identity text DEFAULT 'github.com/example/repo',ADD COLUMN repository_url text DEFAULT 'https://github.com/example/repo',ADD COLUMN display_name text DEFAULT 'example/repo',ADD COLUMN default_branch text DEFAULT 'main'; UPDATE api_cloud_project_builds SET settings=settings||'{\"rustSlackVersion\":1}'::jsonb;").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO api_cloud_billing_periods(period_id,account_id,status,period_start,period_end,included_provider_cost_micros,markup_basis_points,overage_cap_micros,provider_subscription_id) VALUES('period','user-bob','active',$1,$2,100000,500,50000,'subscription')").bind(now()-1000).bind(now()+86_400_000).execute(&pool).await.unwrap();
    let key = B64.encode([27; 32]);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[26; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":B64.encode([26;32]),"x":B64.encode(signing.verifying_key().as_bytes())});
    let runtime = crate::runtime::RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&key).unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api-rust.zuse.sh")
            .unwrap(),
    )
    .with_creation_accounts(vec!["user-bob".into(), "organization:org-team".into()]);
    let app = AppState {
        store: crate::store::Store::from_pool(pool.clone()),
        identity: Arc::new(Identity),
        organizations_enabled: true,
        organization_ids: Arc::new(vec!["org-team".into()]),
        runtime: Some(Arc::new(runtime)),
        gateway: Arc::new(crate::gateway::Hub::default()),
    };
    let calls = Arc::new(std::sync::Mutex::new(vec![]));
    let observed = calls.clone();
    let mock=Router::new().route("/{method}",axum::routing::get(mock_get).post(move|axum::extract::Path(method):axum::extract::Path<String>,bytes:axum::body::Bytes|{let calls=observed.clone();async move{
        let body:Value=serde_json::from_slice(&bytes).unwrap_or_else(|_|json!({"form":String::from_utf8_lossy(&bytes).to_string()}));
        let deleted=method=="chat.update" && body["ts"]=="deleted-reply";
        calls.lock().unwrap().push((method.clone(),body));
        axum::Json(if deleted {json!({"ok":false,"error":"message_not_found"})}else if method=="oauth.v2.access"{json!({"ok":true,"team":{"id":"T_WORKSPACE","name":"Engineering"},"authed_user":{"id":"U_ADA"},"bot_user_id":"U_ZUSE","access_token":"xoxb-test-only"})}else{json!({"ok":true,"ts":"1793296801.000001"})})
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });
    let slack = Slack {
        app,
        pool: pool.clone(),
        master: Arc::new(MasterKey::parse(&key).unwrap()),
        config: Arc::new(Config {
            signing_secret: "signing-fixture".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            redirect_uri: "https://api-rust.zuse.sh/slack/oauth/callback".into(),
        }),
        http: reqwest::Client::new(),
        api_origin: origin,
        retry_after: Arc::new(std::sync::atomic::AtomicI64::new(0)),
    };
    (admin, pool, schema, slack, calls)
}
async fn mock_get(axum::extract::Path(method): axum::extract::Path<String>) -> axum::Json<Value> {
    axum::Json(match method.as_str() {
        "users.info" => json!({"ok":true,"user":{"profile":{"display_name":"Ada Lovelace"}}}),
        "conversations.info" => json!({"ok":true,"channel":{"name":"engineering"}}),
        _ => json!({"ok":true}),
    })
}
async fn finish(admin: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
async fn link(slack: &Slack) {
    sqlx::query("INSERT INTO zuse_rust_slack_installs VALUES('T_WORKSPACE','Engineering','U_ZUSE',$1,'generation')").bind(slack.seal("slack-install:T_WORKSPACE",&json!({"token":"xoxb-test-only"})).unwrap()).execute(&slack.pool).await.unwrap();
    sqlx::query("INSERT INTO zuse_rust_slack_members VALUES('T_WORKSPACE','U_ADA','user-bob','user-bob',NULL,$1,'revision')").bind(json!({"projectId":"create-project","agent":"claude-code","model":"claude-sonnet-5"})).execute(&slack.pool).await.unwrap();
}
async fn ingress(slack: &Slack, body: &str) {
    let headers = signed(body.as_bytes(), now() / 1000);
    let mut builder = Request::post("/slack/events")
        .header("content-type", "application/json")
        .header("x-slack-retry-num", "1");
    *builder.headers_mut().unwrap() = headers;
    let response = slack
        .clone()
        .router()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
async fn due(slack: &Slack) {
    sqlx::query("UPDATE zuse_rust_slack_jobs SET next_at=0 WHERE phase<>'done'")
        .execute(&slack.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_signed_retries_restart_create_one_run_and_preserve_source() {
    let (admin, pool, schema, mut slack, calls) = fixture().await;
    link(&slack).await;
    ingress(&slack, MENTION).await;
    ingress(&slack, MENTION).await;
    let mut duplicate: Value = serde_json::from_str(MENTION).unwrap();
    duplicate["event_id"] = json!("Ev_DUPLICATE_DELIVERY");
    ingress(&slack, &duplicate.to_string()).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    for _ in 0..3 {
        due(&slack).await;
        slack.clone().work_once().await.unwrap();
    }
    // Simulate a crash after external enqueue but before recording the next phase.
    sqlx::query("UPDATE zuse_rust_slack_jobs SET phase='enqueue',next_at=0")
        .execute(&pool)
        .await
        .unwrap();
    slack.clone().work_once().await.unwrap();
    let counts:(i64,i64)=sqlx::query_as("SELECT (SELECT COUNT(*) FROM api_cloud_workspaces WHERE idempotency_key LIKE 'slack-thread:%'),(SELECT COUNT(*) FROM zuse_rust_mailbox_commands)").fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (1, 1));
    let envelope: Value = sqlx::query_scalar("SELECT envelope FROM zuse_rust_mailbox_commands")
        .fetch_one(&pool)
        .await
        .unwrap();
    let envelope: zuse_protocol::cloud_commands::CommandEnvelope =
        serde_json::from_value(envelope).unwrap();
    let access = Access {
        actor: Principal {
            account_id: "user-bob".into(),
        },
        scope: crate::access::Scope::Personal,
        owner_id: "user-bob".into(),
        membership: None,
    };
    let data = slack
        .app
        .runtime
        .as_ref()
        .unwrap()
        .command_data_key(&envelope.workspace_id, &access)
        .await
        .unwrap();
    let key = zuse_cloud_runtime::mailbox::CommandKey::parse(data["encodedKey"].as_str().unwrap())
        .unwrap();
    let bytes = key.decrypt(&envelope).unwrap();
    let payload: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["source"]["author"], "Ada Lovelace");
    assert_eq!(payload["source"]["channel"], "engineering");
    assert_eq!(payload["source"]["text"], "Fix the failing test");
    assert!(
        payload["text"]
            .as_str()
            .unwrap()
            .contains("Request from Slack")
    );
    let private: String = sqlx::query_scalar("SELECT payload FROM zuse_rust_slack_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!private.contains("Fix the failing test"));
    assert!(
        !serde_json::to_string(
            &request(&slack.clone().router(), "/v1/cloud/slack", None)
                .await
                .1
        )
        .unwrap()
        .contains("xoxb")
    );
    sqlx::raw_sql(include_str!("../../tests/fixtures/transcript.sql"))
        .execute(&pool)
        .await
        .unwrap();
    publish_result(
        &mut slack,
        &envelope.workspace_id,
        &envelope.session_id,
        data["encodedKey"].as_str().unwrap(),
        "unrelated-message",
        1,
    )
    .await;
    due(&slack).await;
    slack.work_once().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT phase FROM zuse_rust_slack_jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "poll",
        "Unrelated completed turns must not be delivered"
    );
    publish_result(
        &mut slack,
        &envelope.workspace_id,
        &envelope.session_id,
        data["encodedKey"].as_str().unwrap(),
        payload["clientMessageId"].as_str().unwrap(),
        2,
    )
    .await;
    due(&slack).await;
    slack.work_once().await.unwrap();
    due(&slack).await;
    slack.work_once().await.unwrap();
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "chat.postMessage")
            .count(),
        1
    );
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|(method, body)| method == "chat.update" && body["text"] == "Fixed the test.")
    );
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_oauth_is_browser_bound_single_use_and_disconnect_is_member_scoped() {
    let (admin, pool, schema, slack, calls) = fixture().await;
    let app = slack.clone().router();
    let(status,begin)=request(&app,"/v1/cloud/slack/connect",Some(json!({"projectId":"create-project","agent":"claude-code","model":"claude-sonnet-5","workspaceId":"personal"}))).await;
    assert_eq!(status, StatusCode::OK, "{begin}");
    let url = reqwest::Url::parse(begin["url"].as_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/slack/oauth/start?state={state}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = String::from_utf8(
        to_bytes(response.into_body(), 65536)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("user-bob"));
    assert!(html.contains("Choose Slack workspace"));
    for origin in ["https://evil.example", "https://api-rust.zuse.sh"] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/slack/oauth/authorize")
                    .header("origin", origin)
                    .header("cookie", &cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("state={state}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if origin.contains("evil") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::SEE_OTHER
            }
        );
    }
    let callback = format!("/slack/oauth/callback?state={state}&code=test-code");
    let response = app
        .clone()
        .oneshot(
            Request::get(&callback)
                .header("cookie", "zuse_slack=other-browser")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::get(&callback)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(
            Request::get(&callback)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "oauth.v2.access")
            .count(),
        1
    );
    {
        let calls = calls.lock().unwrap();
        let form = calls
            .iter()
            .find(|(method, _)| method == "oauth.v2.access")
            .unwrap()
            .1["form"]
            .as_str()
            .unwrap();
        assert!(form.contains("client_id=client"));
        assert!(form.contains("client_secret=secret"));
        assert!(form.contains("code=test-code"));
    }
    // Finishing Slack sign-in in a browser links nothing by itself.
    let (_, listed) = request(&app, "/v1/cloud/slack", None).await;
    assert_eq!(listed["connections"], json!([]));
    assert_eq!(listed["pending"][0]["teamName"], "Engineering");
    assert_eq!(listed["pending"][0]["slackUserId"], "U_ADA");
    assert_eq!(listed["pending"][0]["slackUserName"], "Ada Lovelace");
    let pending = listed["pending"][0]["pendingId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = request(
        &app,
        "/v1/cloud/slack/confirm",
        Some(json!({"pendingId":"forged","accept":true})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, confirmed) = request(
        &app,
        "/v1/cloud/slack/confirm",
        Some(json!({"pendingId":pending,"accept":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["confirmed"], true);
    // Single use: the same confirmation cannot be replayed.
    let (status, _) = request(
        &app,
        "/v1/cloud/slack/confirm",
        Some(json!({"pendingId":pending,"accept":true})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, listed) = request(&app, "/v1/cloud/slack", None).await;
    assert_eq!(listed["connections"][0]["teamName"], "Engineering");
    assert_eq!(listed["pending"], json!([]));
    assert!(!listed.to_string().contains("xoxb"));
    assert_eq!(listed["workspaces"][0]["id"], "personal");
    sqlx::query("INSERT INTO zuse_rust_slack_members(team_id,user_id,account_id,actor_id,organization_id,settings,revision) SELECT team_id,'U_OTHER','other-account','other-account',organization_id,settings,'other-revision' FROM zuse_rust_slack_members WHERE user_id='U_ADA'")
        .execute(&pool).await.unwrap();
    let (status, _) = request(
        &app,
        "/v1/cloud/slack/disconnect",
        Some(json!({"teamId":"T_WORKSPACE","slackUserId":"U_ADA"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        request(&app, "/v1/cloud/slack", None).await.1["connections"],
        json!([])
    );
    let remaining: (i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM zuse_rust_slack_members WHERE user_id='U_OTHER'),(SELECT COUNT(*) FROM zuse_rust_slack_installs)")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(remaining, (1, 1));
    drop(app);
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_connection_rejects_unprepared_models_expiry_and_forged_scope() {
    let (admin, pool, schema, slack, _) = fixture().await;
    let app = slack.clone().router();
    assert_eq!(
        request(
            &app,
            "/v1/cloud/slack/connect",
            Some(json!({"projectId":"create-project","agent":"mock","model":"fake"}))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(request(&app,"/v1/cloud/slack/connect",Some(json!({"projectId":"private-project","agent":"claude-code","model":"claude-sonnet-5"}))).await.0,StatusCode::CONFLICT);
    assert_eq!(request(&app,"/v1/cloud/slack/connect",Some(json!({"projectId":"create-project","agent":"claude-code","model":"claude-sonnet-5","workspaceId":"organization:forged"}))).await.0,StatusCode::FORBIDDEN);
    sqlx::query("UPDATE api_cloud_project_builds SET settings=settings-'rustSlackVersion'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(request(&app,"/v1/cloud/slack/connect",Some(json!({"projectId":"create-project","agent":"claude-code","model":"claude-sonnet-5"}))).await.0,StatusCode::CONFLICT);
    sqlx::query("INSERT INTO zuse_rust_slack_oauth(state_hash,browser_hash,payload,expires_at)VALUES($1,'','expired',0)").bind(hash("expired")).execute(&pool).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/slack/oauth/start?state=expired")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(Request::get("/v1/cloud/slack").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    drop(app);
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_followups_reuse_the_thread_and_account_changes_cannot_take_it_over() {
    let (admin, pool, schema, slack, _) = fixture().await;
    link(&slack).await;
    ingress(&slack, MENTION).await;
    for _ in 0..3 {
        due(&slack).await;
        slack.work_once().await.unwrap();
    }
    sqlx::query("UPDATE zuse_rust_slack_jobs SET phase='done'")
        .execute(&pool)
        .await
        .unwrap();
    ingress(&slack, THREAD).await;
    for _ in 0..3 {
        due(&slack).await;
        slack.work_once().await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_threads")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_mailbox_commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    sqlx::raw_sql("UPDATE zuse_rust_slack_jobs SET phase='done';UPDATE zuse_rust_slack_members SET account_id='different-account',actor_id='different-account'").execute(&pool).await.unwrap();
    let mut other: Value = serde_json::from_str(THREAD).unwrap();
    other["event_id"] = json!("Ev_OTHER");
    other["event"]["ts"] = json!("1793296910.000400");
    ingress(&slack, &other.to_string()).await;
    due(&slack).await;
    slack.work_once().await.unwrap();
    due(&slack).await;
    assert_eq!(
        slack.work_once().await.unwrap_err().code,
        "slack_thread_account_mismatch"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_mailbox_commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_bad_signatures_and_unknown_members_never_start_runs() {
    let (admin, pool, schema, slack, calls) = fixture().await;
    link(&slack).await;
    let response = slack
        .clone()
        .router()
        .oneshot(
            Request::post("/slack/events")
                .body(Body::from(MENTION))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let mut event: Value = serde_json::from_str(MENTION).unwrap();
    event["event"]["user"] = json!("U_UNKNOWN");
    ingress(&slack, &event.to_string()).await;
    slack.work_once().await.unwrap();
    assert!(calls.lock().unwrap().iter().any(|(_, body)| {
        body["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Connect your Slack account")
    }));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_mailbox_commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    drop(slack);
    finish(admin, pool, schema).await;
}

struct OneObject {
    key: String,
    bytes: Vec<u8>,
}
#[async_trait::async_trait]
impl crate::objects::Objects for OneObject {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok((key == self.key).then(|| self.bytes.clone()))
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        assert_eq!(key, self.key);
        assert_eq!(bytes, self.bytes);
        Ok(())
    }
}
async fn publish_result(
    slack: &mut Slack,
    workspace: &str,
    session: &str,
    key: &str,
    message: &str,
    version: i64,
) {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    let cursor = zuse_cloud_projection::Cursor {
        epoch: "slack-epoch".into(),
        version,
    };
    let mut results = serde_json::Map::new();
    results.insert(
        message.into(),
        json!({"completedAt":now(),"text":"Fixed the test.","status":"completed"}),
    );
    let head:zuse_cloud_projection::Head=serde_json::from_value(json!({"schemaVersion":1,"payloadFormat":zuse_cloud_projection::FORMAT,"workspaceId":workspace,"sessionId":session,"runtimeGeneration":1,"cursor":cursor,"chat":{"id":session,"deviceId":"runtime","archived":false,"createdAt":"2026-10-29T00:00:00Z"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false,"olderMessageSequence":null,"slackResults":results})).unwrap();
    let key = zuse_cloud_projection::TranscriptKey::parse(key).unwrap();
    let upload = key.seal(workspace, session, &cursor, None, &head).unwrap();
    let object = crate::transcript::formatted_key(workspace, session, 1, &cursor, None, true);
    sqlx::query("INSERT INTO api_cloud_transcript_checkpoints VALUES($1,$2,1,$3,$4,$5,$6,$7,$8) ON CONFLICT(workspace_id,session_id) DO UPDATE SET stream_version=EXCLUDED.stream_version,object_key=EXCLUDED.object_key,ciphertext_sha256=EXCLUDED.ciphertext_sha256,ciphertext_bytes=EXCLUDED.ciphertext_bytes").bind(workspace).bind(session).bind(&cursor.epoch).bind(version).bind(&object).bind(&upload.ciphertext_sha256).bind(upload.ciphertext.len() as i64).bind(now()).execute(&slack.pool).await.unwrap();
    slack.app.store = slack.app.store.clone().with_transcripts(
        Arc::new(OneObject {
            key: object,
            bytes: upload.ciphertext.into_bytes(),
        }),
        MasterKey::parse(&B64.encode([27; 32])).unwrap(),
    );
}

struct OrganizationIdentity {
    revoked: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait::async_trait]
impl crate::auth::Identity for OrganizationIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        Identity.verify(token).await
    }
    async fn membership(
        &self,
        account: &str,
        organization: &str,
    ) -> Result<crate::auth::Membership, ApiError> {
        if self.revoked.load(std::sync::atomic::Ordering::Relaxed)
            || account != "user-bob"
            || organization != "org-team"
        {
            return Err(ApiError::forbidden("membership_revoked"));
        }
        Ok(crate::auth::Membership {
            id: "member-bob".into(),
            user_id: account.into(),
            organization_id: organization.into(),
            status: "active".into(),
            role: crate::auth::MembershipRole {
                slug: "member".into(),
            },
        })
    }
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_organization_picker_preserves_scope_and_does_not_allocate_unsupported_runtimes() {
    let (admin, pool, schema, mut slack, _) = fixture().await;
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    slack.app.identity = Arc::new(OrganizationIdentity {
        revoked: revoked.clone(),
    });
    let (_, listed) = request(&slack.clone().router(), "/v1/cloud/slack", None).await;
    assert_eq!(
        listed["workspaces"],
        json!([{"id":"personal","name":"Personal"},{"id":"organization:org-team","name":"org-team"}])
    );
    let app = slack.clone().router();
    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/cloud/slack")
                .header("authorization", "Bearer bob")
                .header("x-zuse-workspace", "organization:org-team")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let scoped: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(scoped["workspaceId"], "organization:org-team");
    assert_eq!(scoped["executionAvailable"], false);
    let response=app.clone().oneshot(Request::post("/v1/cloud/slack/connect").header("authorization","Bearer bob").header("x-zuse-workspace","organization:org-team").header("content-type","application/json").body(Body::from(json!({"workspaceId":"organization:org-team","projectId":"create-project","agent":"claude-code","model":"claude-sonnet-5"}).to_string())).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_oauth")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_allocations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    revoked.store(true, std::sync::atomic::Ordering::Relaxed);
    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/cloud/slack")
                .header("authorization", "Bearer bob")
                .header("x-zuse-workspace", "organization:org-team")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    drop(app);
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_signed_challenge_and_uninstall_do_not_create_work() {
    let (admin, pool, schema, slack, calls) = fixture().await;
    link(&slack).await;
    let challenge = json!({"type":"url_verification","challenge":"challenge-fixture"}).to_string();
    let mut builder = Request::post("/slack/events");
    *builder.headers_mut().unwrap() = signed(challenge.as_bytes(), now() / 1000);
    let response = slack
        .clone()
        .router()
        .oneshot(builder.body(Body::from(challenge)).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["challenge"], "challenge-fixture");
    ingress(&slack, MENTION).await;
    ingress(&slack,&json!({"type":"event_callback","team_id":"T_WORKSPACE","event":{"type":"app_uninstalled"}}).to_string()).await;
    slack.work_once().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_installs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_members")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert!(calls.lock().unwrap().is_empty());
    drop(slack);
    finish(admin, pool, schema).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_lost_creation_response_reuses_the_frozen_thread_intent() {
    let (admin, pool, schema, slack, _) = fixture().await;
    link(&slack).await;
    ingress(&slack, MENTION).await;
    slack.work_once().await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION fail_slack_thread_map() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture lost create response'; END $$; CREATE TRIGGER fail_slack_thread_map BEFORE UPDATE ON zuse_rust_slack_threads FOR EACH ROW EXECUTE FUNCTION fail_slack_thread_map();").execute(&pool).await.unwrap();
    due(&slack).await;
    assert!(slack.work_once().await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM api_cloud_workspaces WHERE idempotency_key LIKE 'slack-thread:%'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    sqlx::raw_sql("DROP TRIGGER fail_slack_thread_map ON zuse_rust_slack_threads; DROP FUNCTION fail_slack_thread_map(); UPDATE api_cloud_projects SET default_branch='develop';").execute(&pool).await.unwrap();
    due(&slack).await;
    slack.work_once().await.unwrap();
    due(&slack).await;
    slack.work_once().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM api_cloud_workspaces WHERE idempotency_key LIKE 'slack-thread:%'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_mailbox_commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT base_ref FROM api_cloud_workspaces WHERE idempotency_key LIKE 'slack-thread:%'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "main"
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT creation_request IS NULL FROM zuse_rust_slack_threads"
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    drop(slack);
    finish(admin, pool, schema).await;
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_deleted_progress_reply_is_replaced_before_result_delivery() {
    let (admin, pool, schema, slack, calls) = fixture().await;
    link(&slack).await;
    ingress(&slack, MENTION).await;
    slack.work_once().await.unwrap();
    let encoded: String = sqlx::query_scalar("SELECT payload FROM zuse_rust_slack_jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut event: Event = slack
        .open(&job_context("T_WORKSPACE", "Ev_MENTION"), &encoded)
        .unwrap();
    event.progress_ts = Some("deleted-reply".into());
    event.result = Some("Recovered final result".into());
    slack.save(&event, "finish", 0).await.unwrap();
    slack.work_once().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT phase FROM zuse_rust_slack_jobs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "done"
    );
    let calls = calls.lock().unwrap();
    let posted = calls
        .iter()
        .filter(|(method, _)| method == "chat.postMessage")
        .collect::<Vec<_>>();
    assert_eq!(posted.len(), 2);
    assert_ne!(posted[0].1["client_msg_id"], posted[1].1["client_msg_id"]);
    assert_eq!(posted[1].1["text"], "Recovered final result");
    drop(calls);
    drop(slack);
    finish(admin, pool, schema).await;
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_queue_gives_older_due_new_work_priority_over_rescheduled_polls() {
    let (admin, pool, schema, slack, _) = fixture().await;
    link(&slack).await;
    for index in 0..3 {
        let mut dm: Value = serde_json::from_str(DM).unwrap();
        dm["event_id"] = json!(format!("Ev_OLD_{index}"));
        dm["event"]["ts"] = json!(format!("1793296810.00000{index}"));
        ingress(&slack, &dm.to_string()).await;
    }
    sqlx::query("UPDATE zuse_rust_slack_jobs SET phase='poll',next_at=$1")
        .bind(now() - 1000)
        .execute(&pool)
        .await
        .unwrap();
    ingress(&slack, MENTION).await;
    sqlx::query("UPDATE zuse_rust_slack_jobs SET next_at=$1 WHERE event_id='Ev_MENTION'")
        .bind(now() - 10_000)
        .execute(&pool)
        .await
        .unwrap();
    assert!(slack.work_once().await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT phase FROM zuse_rust_slack_jobs WHERE event_id='Ev_MENTION'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "dispatch"
    );
    drop(slack);
    finish(admin, pool, schema).await;
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn slack_explicit_dm_thread_mentions_continue_the_existing_run() {
    let (admin, pool, schema, slack, _) = fixture().await;
    link(&slack).await;
    ingress(&slack, DM).await;
    for _ in 0..3 {
        due(&slack).await;
        slack.work_once().await.unwrap();
    }
    sqlx::query("UPDATE zuse_rust_slack_jobs SET phase='done'")
        .execute(&pool)
        .await
        .unwrap();
    let mut dm: Value = serde_json::from_str(DM).unwrap();
    dm["event_id"] = json!("Ev_DM_FOLLOWUP");
    dm["event"]["thread_ts"] = dm["event"]["ts"].clone();
    dm["event"]["ts"] = json!("1793296811.000200");
    dm["event"]["text"] = json!("<@U_ZUSE> Continue this task");
    ingress(&slack, &dm.to_string()).await;
    for _ in 0..3 {
        due(&slack).await;
        slack.work_once().await.unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_slack_threads")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM zuse_rust_mailbox_commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    drop(slack);
    finish(admin, pool, schema).await;
}

#[test]
fn slack_official_signature_example_matches_without_using_the_test_signer() {
    // Public example from Slack's request verification documentation.
    let mut headers = HeaderMap::new();
    headers.insert("x-slack-request-timestamp", "1531420618".parse().unwrap());
    headers.insert(
        "x-slack-signature",
        "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503"
            .parse()
            .unwrap(),
    );
    assert!(
        http::verify(
            "8f742231b10e8888abcd99yyyzzz85a5",
            &headers,
            include_bytes!("../../tests/fixtures/slack/official-hmac.form"),
            1531420618
        )
        .is_ok()
    );
}
