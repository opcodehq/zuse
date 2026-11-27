//! Ownership, webhook routing and the inbox against a real PostgreSQL.
//! Run with `ZUSE_CLOUD_TEST_DATABASE_URL` set and `--include-ignored`.
use super::*;
use hmac::Mac;

const SECRET: &str = "webhook-secret";
const ACCOUNT: &str = "actor-one";

async fn database() -> (PgPool, PgPool, String) {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
    assert!(matches!(
        reqwest::Url::parse(&url).unwrap().host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("pull_requests_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let search = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
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
    sqlx::raw_sql(include_str!("../../../../migrations/0015_github.sql"))
        .execute(&pool)
        .await
        .unwrap();
    // The account event outbox and the shared hint trigger function, then
    // the migration under test.
    let (outbox, _) = include_str!("../../../../migrations/0017_account_events.sql")
        .split_once("CREATE TABLE zuse_rust_account_event_cursors")
        .unwrap();
    sqlx::raw_sql(outbox).execute(&pool).await.unwrap();
    let hints = include_str!("../../../../migrations/0049_change_hints.sql");
    let start = hints
        .find("CREATE OR REPLACE FUNCTION zuse_rust_change_hint_outbox")
        .unwrap();
    let end = start + hints[start..].find("END $body$;").unwrap() + "END $body$;".len();
    sqlx::raw_sql(&hints[start..end])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/0058_pull_request_owners.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    (admin, pool, schema)
}

fn authority(pool: &PgPool) -> NativeGithub {
    use rsa::pkcs1::EncodeRsaPrivateKey;
    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let der = key.to_pkcs1_der().unwrap();
    let origin = reqwest::Url::parse("http://127.0.0.1:1/").unwrap();
    NativeGithub {
        membership_epoch: std::sync::atomic::AtomicU64::new(0),
        observation_gates: Default::default(),
        fast_start_accounts: crate::access_policy::Accounts::listed([]),
        pool: pool.clone(),
        master: MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
        accounts: crate::access_policy::Accounts::listed([ACCOUNT.into()]),
        http: reqwest::Client::new(),
        config: Config {
            origin: reqwest::Url::parse("https://api.example.test/").unwrap(),
            app_id: "123".into(),
            slug: "fixture".into(),
            client_id: "client".into(),
            client_secret: Zeroizing::new("client-secret".into()),
            webhook_secret: Zeroizing::new(SECRET.into()),
            signing_key: jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()),
            api_origin: origin.clone(),
            oauth_origin: origin,
        },
    }
}

fn access(actor: &str) -> Access {
    Access {
        actor: crate::auth::Principal {
            account_id: actor.into(),
        },
        owner_id: actor.into(),
        scope: crate::access::Scope::Personal,
        membership: None,
    }
}

fn pull_request(number: i64, sha: &str) -> Value {
    json!({
        "number": number,
        "html_url": format!("https://github.com/acme/app/pull/{number}"),
        "title": "Fix the parser",
        "state": "open",
        "merged": false,
        "draft": false,
        "head": {"ref": "fix-parser", "sha": sha},
        "base": {"repo": {"id": 44, "full_name": "acme/app"}}
    })
}

fn claim(chat: &str, number: u64, source: ClaimSource) -> ClaimRequest {
    ClaimRequest {
        chat_key: format!("chat:{chat}"),
        repository: "acme/app".into(),
        number: Some(number),
        branch: None,
        source,
    }
}

async fn deliver(authority: &NativeGithub, event: &str, delivery: &str, body: Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
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
        .webhook(&signature, delivery, event, &body)
        .await
        .unwrap();
}

fn comment(id: i64, number: i64, login: &str, user_id: i64, body: &str) -> Value {
    json!({
        "action": "created",
        "installation": {"id": 9},
        "repository": {"id": 44, "full_name": "acme/app"},
        "sender": {"login": login, "id": user_id},
        "issue": {"number": number, "pull_request": {}},
        "comment": {"id": id, "body": body, "html_url": format!("https://github.com/acme/app/pull/{number}#issuecomment-{id}"), "user": {"login": login, "id": user_id, "type": "User"}}
    })
}

async fn hints(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM zuse_rust_account_event_outbox WHERE kind='pull-request'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn owned_pull_requests_receive_their_events() {
    let (admin, pool, schema) = database().await;
    let authority = authority(&pool);
    let me = access(ACCOUNT);
    // The owner's own GitHub identity, for recognising their own comments.
    sqlx::query("INSERT INTO zuse_rust_github_users(actor_id,github_user_id,state,updated_at) VALUES($1,500,'connected',0)")
        .bind(ACCOUNT).execute(&pool).await.unwrap();

    let owned = authority
        .store_claim(
            &me,
            &claim("one", 7, ClaimSource::Created),
            &pull_request(7, "aaaaaaa1"),
            9,
        )
        .await
        .unwrap();
    assert_eq!(owned.chat_key, "chat:one");
    assert_eq!(owned.repository, "acme/app");
    assert_eq!(owned.state, "open");
    assert_eq!(owned.head_ref.as_deref(), Some("fix-parser"));
    assert!(hints(&pool).await >= 1, "a claim tells the account's apps");

    // A branch claim never takes a pull request another chat owns.
    let kept = authority
        .store_claim(
            &me,
            &claim("two", 7, ClaimSource::Branch),
            &pull_request(7, "aaaaaaa1"),
            9,
        )
        .await
        .unwrap();
    assert_eq!(kept.chat_key, "chat:one");

    // Someone else's comment wakes the owner; my own (the agent's token) is
    // only context; a bot's is dropped; a different installation is ignored.
    deliver(
        &authority,
        "issue_comment",
        "d1",
        comment(1, 7, "bob", 2, "Please rename this"),
    )
    .await;
    deliver(
        &authority,
        "issue_comment",
        "d2",
        comment(2, 7, "me", 500, "Renamed"),
    )
    .await;
    let mut bot = comment(3, 7, "lint[bot]", 3, "Style nits");
    bot["comment"]["user"]["type"] = json!("Bot");
    deliver(&authority, "issue_comment", "d3", bot).await;
    let mut foreign = comment(4, 7, "eve", 4, "Hi");
    foreign["installation"]["id"] = json!(10);
    deliver(&authority, "issue_comment", "d4", foreign).await;
    // A retried delivery records nothing twice.
    deliver(
        &authority,
        "issue_comment",
        "d1",
        comment(1, 7, "bob", 2, "Please rename this"),
    )
    .await;
    // Unowned pull requests are left alone.
    deliver(
        &authority,
        "issue_comment",
        "d5",
        comment(5, 8, "bob", 2, "Elsewhere"),
    )
    .await;

    let inbox = authority.pull_request_inbox(&me).await.unwrap();
    let summary: Vec<_> = inbox
        .iter()
        .map(|e| {
            (
                e.chat_key.as_str(),
                e.kind,
                e.action,
                e.body.as_str(),
                e.author.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "chat:one",
                PullRequestEventKind::Comment,
                EventAction::Wake,
                "Please rename this",
                Some("bob")
            ),
            (
                "chat:one",
                PullRequestEventKind::Comment,
                EventAction::Context,
                "Renamed",
                Some("me")
            ),
        ]
    );
    assert_eq!(inbox[0].repository, "acme/app");
    assert_eq!(inbox[0].number, 7);
    assert!(inbox[0].url.ends_with("#issuecomment-1"));

    // Another member's inbox is empty.
    sqlx::query(
        "INSERT INTO zuse_rust_github_scopes(owner_id) VALUES('intruder') ON CONFLICT DO NOTHING",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Check failures wake; a pass on the next commit is reported once; a
    // pass with no earlier failure is not news.
    let failed = json!({
        "action": "completed", "installation": {"id": 9}, "repository": {"id": 44},
        "check_run": {"id": 70, "name": "test", "conclusion": "failure", "head_sha": "aaaaaaa1",
            "details_url": "https://github.com/acme/app/actions/runs/1", "output": {"title": "1 failed"},
            "pull_requests": []}
    });
    deliver(&authority, "check_run", "d6", failed).await;
    let push = json!({
        "action": "synchronize", "installation": {"id": 9}, "repository": {"id": 44},
        "sender": {"login": "me", "id": 500},
        "pull_request": {"number": 7, "title": "Fix the parser", "state": "open", "merged": false,
            "draft": false, "html_url": "https://github.com/acme/app/pull/7",
            "head": {"ref": "fix-parser", "sha": "bbbbbbb2"}}
    });
    deliver(&authority, "pull_request", "d7", push).await;
    let suite = |id: i64, sha: &str| {
        json!({"action": "completed", "installation": {"id": 9}, "repository": {"id": 44},
            "check_suite": {"id": id, "conclusion": "success", "head_sha": sha, "pull_requests": [{"number": 7}]}})
    };
    deliver(&authority, "check_suite", "d8", suite(80, "bbbbbbb2")).await;
    deliver(&authority, "check_suite", "d9", suite(81, "bbbbbbb2")).await;

    let inbox = authority.pull_request_inbox(&me).await.unwrap();
    let kinds: Vec<_> = inbox.iter().map(|e| (e.kind, e.action)).collect();
    assert_eq!(
        kinds,
        vec![
            (PullRequestEventKind::Comment, EventAction::Wake),
            (PullRequestEventKind::Comment, EventAction::Context),
            (PullRequestEventKind::ChecksFailed, EventAction::Wake),
            (PullRequestEventKind::ChecksPassed, EventAction::Wake),
        ]
    );
    assert_eq!(inbox[2].body, "test failed\n\n1 failed");
    let listed = authority.owned_pull_requests(&me).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].checks.as_deref(), Some("passed"));

    // Acknowledged events are gone; acknowledging someone else's is a no-op.
    let first: Vec<i64> = inbox[..2].iter().map(|e| e.sequence).collect();
    assert_eq!(
        authority
            .ack_pull_request_inbox(&access("intruder"), &first)
            .await
            .unwrap_or(0),
        0
    );
    assert_eq!(
        authority.ack_pull_request_inbox(&me, &first).await.unwrap(),
        2
    );
    assert_eq!(authority.pull_request_inbox(&me).await.unwrap().len(), 2);

    // A push from another chat takes the pull request over, and pending
    // events go with it.
    let moved = authority
        .store_claim(
            &me,
            &claim("two", 7, ClaimSource::Pushed),
            &pull_request(7, "bbbbbbb2"),
            9,
        )
        .await
        .unwrap();
    assert_eq!(moved.chat_key, "chat:two");
    assert!(
        authority
            .pull_request_inbox(&me)
            .await
            .unwrap()
            .iter()
            .all(|e| e.chat_key == "chat:two")
    );

    // Event settings: merges wake once asked to.
    let mut policy = EventPolicy::default();
    policy
        .actions
        .insert(PullRequestEventKind::Merged, EventAction::Wake);
    authority
        .set_pull_request_policy(&me, "chat:two", "acme/app", 7, &policy)
        .await
        .unwrap();
    let merged = json!({
        "action": "closed", "installation": {"id": 9}, "repository": {"id": 44},
        "sender": {"login": "bob", "id": 2},
        "pull_request": {"number": 7, "title": "Fix the parser", "state": "closed", "merged": true,
            "draft": false, "updated_at": "2026-11-27T12:00:00Z", "html_url": "https://github.com/acme/app/pull/7",
            "head": {"ref": "fix-parser", "sha": "bbbbbbb2"}}
    });
    deliver(&authority, "pull_request", "d10", merged).await;
    let last = authority
        .pull_request_inbox(&me)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        (last.kind, last.action),
        (PullRequestEventKind::Merged, EventAction::Wake)
    );
    assert_eq!(
        authority.owned_pull_requests(&me).await.unwrap()[0].state,
        "merged"
    );

    // Only the owning chat can release it; its events go with it.
    assert_eq!(
        authority
            .release_pull_request(&me, "chat:one", "acme/app", 7)
            .await
            .unwrap_err()
            .code,
        "pull_request_not_owned"
    );
    authority
        .release_pull_request(&me, "chat:two", "ACME/app", 7)
        .await
        .unwrap();
    assert!(authority.owned_pull_requests(&me).await.unwrap().is_empty());
    assert!(authority.pull_request_inbox(&me).await.unwrap().is_empty());

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
