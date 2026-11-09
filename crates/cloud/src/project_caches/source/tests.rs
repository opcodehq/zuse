use super::*;
use crate::objects::Objects as _;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
struct Memory {
    data: Mutex<BTreeMap<String, Vec<u8>>>,
    puts: AtomicUsize,
    lose_ack: AtomicBool,
}
#[async_trait]
impl crate::objects::Objects for Memory {
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, ApiError> {
        self.data
            .lock()
            .unwrap()
            .retain(|key, _| !key.starts_with(prefix));
        Ok(true)
    }
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        Ok(self.data.lock().unwrap().get(key).cloned())
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.data.lock().unwrap().insert(key.into(), bytes.to_vec());
        if self.lose_ack.swap(false, Ordering::SeqCst) {
            return Err(ApiError::unavailable("fixture_lost_put_response"));
        }
        Ok(())
    }
}
struct Cipher(crate::crypto::MasterKey);
impl SourceCipher for Cipher {
    fn seal(&self, key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
        self.0
            .seal(key.as_bytes(), bytes)
            .map(String::into_bytes)
            .map_err(|_| ApiError::unavailable("fixture_encryption_failed"))
    }
    fn open(&self, key: &str, bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
        self.0
            .open(key.as_bytes(), std::str::from_utf8(bytes).unwrap_or(""))
            .map(|data| data.to_vec())
            .map_err(|_| ApiError::unavailable("fixture_decryption_failed"))
    }
}
#[test]
fn private_git_helper_is_ephemeral_and_refuses_other_hosts_and_token_export() {
    let path = askpass().unwrap();
    let secret = "fixture-repository-scoped-credential";
    for (prompt, expected) in [
        (
            "Username for 'https://github.com': ",
            Some("x-access-token"),
        ),
        (
            "Password for 'https://x-access-token@github.com': ",
            Some(secret),
        ),
        ("Password for 'https://attacker.example': ", None),
        ("Password for 'http://x-access-token@github.com': ", None),
        ("get", None),
    ] {
        let result = std::process::Command::new(&path)
            .arg(prompt)
            .env_clear()
            .env("ZUSE_PROJECT_FETCH_TOKEN", secret)
            .output()
            .unwrap();
        assert_eq!(result.status.success(), expected.is_some());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            expected.unwrap_or("")
        );
        assert!(result.stderr.is_empty());
    }
    assert!(!std::fs::read_to_string(&path).unwrap().contains(secret));
    let original = path.to_path_buf();
    drop(path);
    assert!(!original.exists());
}
#[tokio::test]
async fn authenticated_fetch_cannot_use_file_transport_or_an_expired_credential() {
    let root = tempfile::tempdir().unwrap();
    let mut credential = SourceCredential {
        token: zeroize::Zeroizing::new("fixture-secret".into()),
        github_user_id: 7,
        expires_at: chrono::Utc::now().timestamp_millis() + 60_000,
        provider_expires_at: chrono::Utc::now().timestamp_millis() + 60_000,
        authorization: "opaque".into(),
    };
    git(root.path(), &["init", "--bare", "."], false)
        .await
        .unwrap();
    let source = root.path().join("valid-source");
    git(root.path(), &["init", source.to_str().unwrap()], false)
        .await
        .unwrap();
    std::fs::write(source.join("README"), "credential-free fixture").unwrap();
    git(&source, &["add", "."], false).await.unwrap();
    git(
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "source",
        ],
        false,
    )
    .await
    .unwrap();
    let url = format!("file://{}", source.display());
    let error = git_authenticated(
        root.path(),
        &["fetch", "--", &url, "HEAD"],
        false,
        Some(&credential),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "project_cache_git_failed");
    credential.expires_at = 0;
    let error = git_authenticated(
        root.path(),
        &[
            "fetch",
            "--",
            "https://github.com/example/repository.git",
            "main",
        ],
        false,
        Some(&credential),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "project_cache_source_credential_expired");
    let config = std::fs::read_to_string(root.path().join("config")).unwrap();
    assert!(!config.contains("fixture-secret"));
    assert!(!config.contains("askpass"));
    assert!(!config.contains("credential"));
}
#[test]
fn only_exact_public_repository_metadata_can_authorize_anonymous_sources() {
    public_metadata(
        "example/repository",
        &json!({"id":1,"private":false,"full_name":"Example/Repository"}),
    )
    .unwrap();
    for value in [
        json!({"id":1,"private":true,"full_name":"example/repository"}),
        json!({"id":1,"private":false,"full_name":"other/repository"}),
        json!({"id":0,"private":false,"full_name":"example/repository"}),
        json!({"id":1,"full_name":"example/repository"}),
    ] {
        assert!(public_metadata("example/repository", &value).is_err());
    }
}
#[tokio::test]
async fn source_bundle_contains_the_exact_full_history_commit_and_repository_recipe() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    git(temp.path(), &["init", source.to_str().unwrap()], false)
        .await
        .unwrap();
    std::fs::write(source.join("README.md"), "retained public source").unwrap();
    git(&source, &["add", "."], false).await.unwrap();
    git(
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "first",
        ],
        false,
    )
    .await
    .unwrap();
    std::fs::create_dir(source.join(".zuse")).unwrap();
    let mut recipe = zuse_protocol::cloud_computers::EnvironmentRecipe::default();
    recipe.scripts.initialize = Some("printf prepared > environment-proof".into());
    std::fs::write(
        source.join(".zuse/setup.json"),
        serde_json::to_vec(&recipe).unwrap(),
    )
    .unwrap();
    std::fs::write(source.join("package-lock.json"), "{}").unwrap();
    git(&source, &["add", "."], false).await.unwrap();
    git(
        &source,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-m",
            "recipe",
        ],
        false,
    )
    .await
    .unwrap();
    let commit = git(&source, &["rev-parse", "HEAD"], true).await.unwrap();
    let bundle = build_bundle(source.to_str().unwrap(), "HEAD", Default::default())
        .await
        .unwrap();
    assert_eq!(bundle.commit, commit);
    assert_eq!(bundle.recipe, recipe);
    assert_eq!(bundle.recipe_source, "repository");
    assert!(bundle.dependency_inputs.contains_key("package-lock.json"));
    assert!(bundle.dependency_inputs.contains_key(".zuse/setup.json"));
    let file = temp.path().join("source.bundle");
    std::fs::write(&file, &bundle.bytes).unwrap();
    let checkout = temp.path().join("checkout");
    git(
        temp.path(),
        &[
            "clone",
            "--no-checkout",
            file.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
        false,
    )
    .await
    .unwrap();
    git(&checkout, &["checkout", "--detach", &commit], false)
        .await
        .unwrap();
    assert_eq!(
        git(&checkout, &["rev-list", "--count", &commit], true)
            .await
            .unwrap(),
        "2"
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join("README.md")).unwrap(),
        "retained public source"
    );
    assert_eq!(
        bundle.sha256,
        format!("{:x}", Sha256::digest(&bundle.bytes))
    );
}
#[tokio::test]
#[ignore = "requires explicit isolated localhost PostgreSQL"]
async fn source_object_intent_survives_lost_put_and_does_not_repeat_or_cross_scope() {
    let url = std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").unwrap();
    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "::1")
    ));
    let admin = PgPool::connect(&url).await.unwrap();
    let schema = format!("source_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let name = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |c, _| {
            let name = name.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path={name}"))
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let original = MIGRATION
        .split("CREATE TABLE zuse_rust_project_source_bundles")
        .nth(1)
        .unwrap()
        .split(";")
        .next()
        .unwrap();
    sqlx::raw_sql(&format!(
        "CREATE TABLE zuse_rust_project_source_bundles{original};"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql("ALTER TABLE zuse_rust_project_source_bundles ADD COLUMN cleanup_due_at bigint,ADD COLUMN cleanup_lease_id text,ADD COLUMN cleanup_lease_expires_at bigint,ADD COLUMN cleanup_attempts integer NOT NULL DEFAULT 0; CREATE TABLE zuse_rust_project_cache_entries(cache_key text,state text);CREATE TABLE zuse_rust_project_cache_operations(owner_id text,project_id text,cache_key text,plan jsonb,state text);CREATE TABLE zuse_rust_project_preparation_requests(owner_id text,project_id text,state text,last_error_code text,lease_expires_at bigint);CREATE TABLE api_cloud_projects(account_id text,project_id text,included bool);INSERT INTO api_cloud_projects VALUES('owner','project',true);").execute(&pool).await.unwrap();
    let objects = Arc::new(Memory {
        data: Mutex::new(BTreeMap::new()),
        puts: AtomicUsize::new(0),
        lose_ack: AtomicBool::new(true),
    });
    use base64::Engine;
    let cipher = Arc::new(Cipher(
        crate::crypto::MasterKey::parse(
            &base64::engine::general_purpose::STANDARD.encode([72; 32]),
        )
        .unwrap(),
    ));
    let store = PublicBundles::new(pool.clone(), objects.clone())
        .unwrap()
        .with_cipher(cipher.clone());
    let bytes = vec![42; CHUNK + 17];
    let sha = format!("{:x}", Sha256::digest(&bytes));
    let bundle = Bundle {
        commit: "c".repeat(40),
        sha256: sha.clone(),
        bytes: bytes.clone(),
        dependency_inputs: Default::default(),
        recipe: Default::default(),
        recipe_source: "project".into(),
    };
    let request = worker::Request {
        request_id: "request".into(),
        owner: "owner".into(),
        actor_id: "owner".into(),
        actor: Value::Null,
        project: "project".into(),
        environment_revision: 1,
        requested_ref: "main".into(),
        repository_identity: "github.com/example/repository".into(),
    };
    assert!(store.store(&request, &bundle, 1).await.is_err());
    let state: String = sqlx::query_scalar("SELECT state FROM zuse_rust_project_source_bundles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "writing");
    store.store(&request, &bundle, 2).await.unwrap();
    assert_eq!(objects.puts.load(Ordering::SeqCst), 2);
    for (key, ciphertext) in objects.data.lock().unwrap().iter() {
        assert!(!ciphertext.windows(256).any(|window| window == [42; 256]));
        assert!(
            cipher
                .open(&format!("other-account/{key}"), ciphertext)
                .is_err()
        );
        let mut changed = ciphertext.clone();
        let n = changed.len() / 2;
        changed[n] ^= 1;
        assert!(cipher.open(key, &changed).is_err());
    }
    assert_eq!(store.load(&sha, "owner", "project").await.unwrap(), bytes);
    assert!(store.load(&sha, "other-owner", "project").await.is_err());
    assert!(store.load(&sha, "owner", "other-project").await.is_err());
    sqlx::query("UPDATE zuse_rust_project_source_bundles SET chunks=jsonb_set(chunks,'{0,key}','\"runtime/other-account/private\"')").execute(&pool).await.unwrap();
    assert_eq!(
        store
            .load(&sha, "owner", "project")
            .await
            .err()
            .unwrap()
            .code,
        "project_cache_source_invalid"
    );
    let late_key = objects.data.lock().unwrap().keys().next().unwrap().clone();
    objects.data.lock().unwrap().insert(
        "runtime/other-account/private".into(),
        b"unrelated fixture".to_vec(),
    );
    sqlx::query(
        "INSERT INTO zuse_rust_project_preparation_requests(owner_id,project_id,state) VALUES('owner','project','resolving')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        !store.reap(86_400_003).await.unwrap(),
        "active original resolver keeps its objects"
    );
    sqlx::query("DELETE FROM zuse_rust_project_preparation_requests")
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.reap(86_400_003).await.unwrap());
    assert_eq!(
        objects.data.lock().unwrap().len(),
        1,
        "only scoped prefix deleted despite corrupt chunk metadata"
    );
    objects
        .put(&late_key, b"late remote PUT after cancelled request")
        .await
        .unwrap();
    assert!(!store.reap(86_400_004).await.unwrap());
    assert!(store.reap(90_000_004).await.unwrap());
    assert_eq!(objects.data.lock().unwrap().len(), 1);
    assert!(
        store.store(&request, &bundle, 90_000_005).await.is_err(),
        "retired source never silently revives"
    );
    let tombstone: (String, i32) =
        sqlx::query_as("SELECT state,cleanup_attempts FROM zuse_rust_project_source_bundles")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tombstone, ("cleaning".into(), 2));
    // Disconnect promptly retires source without the ordinary24h grace; an
    // active original resolver still retains it until fenced or released.
    let fresh = Bundle {
        bytes: vec![43; 17],
        sha256: format!("{:x}", Sha256::digest([43u8; 17])),
        ..bundle
    };
    store.store(&request, &fresh, 1000).await.unwrap();
    sqlx::raw_sql("UPDATE api_cloud_projects SET included=false; INSERT INTO zuse_rust_project_preparation_requests(owner_id,project_id,state,lease_expires_at) VALUES('owner','project','resolving',2000)").execute(&pool).await.unwrap();
    assert!(!store.reap(1001).await.unwrap());
    sqlx::query("UPDATE zuse_rust_project_preparation_requests SET lease_expires_at=1001")
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.reap(1002).await.unwrap());
    assert_eq!(
        objects.data.lock().unwrap().len(),
        1,
        "only unrelated object remains after disconnect"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
