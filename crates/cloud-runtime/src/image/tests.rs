use super::*;
use axum::{
    Router,
    body::Body,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, Response, StatusCode},
    routing::get,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn run(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "fixture git failed");
    String::from_utf8(output.stdout).unwrap().trim().into()
}
#[derive(Clone)]
struct Server {
    root: PathBuf,
    authorized: Arc<AtomicUsize>,
}
async fn serve(
    State(server): State<Server>,
    UrlPath(path): UrlPath<String>,
    headers: HeaderMap,
) -> Response<Body> {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some("Basic eC1hY2Nlc3MtdG9rZW46Zml4dHVyZS1zZWNyZXQ=")
    {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "Basic realm=fixture")
            .body(Body::empty())
            .unwrap();
    }
    server.authorized.fetch_add(1, Ordering::SeqCst);
    if path.split('/').any(|p| p == "..") {
        return Response::builder().status(400).body(Body::empty()).unwrap();
    }
    match tokio::fs::read(server.root.join(path)).await {
        Ok(bytes) => Response::new(Body::from(bytes)),
        Err(_) => Response::builder().status(404).body(Body::empty()).unwrap(),
    }
}
fn plan(root: &Path, url: String) -> ImagePlan {
    ImagePlan {
        build_id: "image-fixture".into(),
        template_version: "rust-fixture-v1".into(),
        configuration_digest: "config-original".into(),
        home: root.join("home"),
        cache_root: root.join("cache"),
        status_directory: root.join("status"),
        secrets_root: root.join("secrets"),
        max_cache_bytes: 16 * 1024 * 1024,
        repositories: vec![ImageRepository {
            project_id: "project-original".into(),
            repository_identity: "127.0.0.1/owner/repo".into(),
            repository_url: url,
            default_branch: "main".into(),
            token_file: Some(root.join("secrets/github-installation-123")),
        }],
    }
}
fn token(plan: &ImagePlan) {
    fs::create_dir_all(&plan.secrets_root).unwrap();
    let file = plan.repositories[0].token_file.as_ref().unwrap();
    fs::write(file, "fixture-secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
    }
}
#[tokio::test]
async fn private_git_image_manifest_is_journaled_and_tokens_are_removed_before_ready() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    run(&source, &["init", "-b", "main"]);
    run(&source, &["config", "user.name", "Fixture"]);
    run(&source, &["config", "user.email", "fixture@example.test"]);
    fs::write(source.join("tracked"), "original bytes").unwrap();
    run(&source, &["add", "tracked"]);
    run(&source, &["commit", "-m", "Original"]);
    let commit = run(&source, &["rev-parse", "HEAD"]);
    let web = root.path().join("web");
    fs::create_dir_all(web.join("owner")).unwrap();
    run(
        &source,
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            web.join("owner/repo.git").to_str().unwrap(),
        ],
    );
    run(&web.join("owner/repo.git"), &["update-server-info"]);
    let authorized = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/{*path}", get(serve))
        .with_state(Server {
            root: web,
            authorized: authorized.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let plan = plan(root.path(), format!("http://{address}/owner/repo.git"));
    token(&plan);
    let copy = plan.clone();
    let manifest = tokio::task::spawn_blocking(move || prepare(&copy))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manifest.repositories[0].source_commit, commit);
    assert!(manifest.credentials_removed);
    assert!(authorized.load(Ordering::SeqCst) > 0);
    assert!(fs::read_dir(&plan.secrets_root).unwrap().next().is_none());
    let output = fs::read_to_string(plan.status_directory.join("manifest.json")).unwrap();
    let log = fs::read_to_string(plan.status_directory.join("build.log")).unwrap();
    assert!(!output.contains("fixture-secret") && !log.contains("fixture-secret"));
    assert!(log.contains("fetching") && log.contains("sanitizing") && log.contains("ready"));
    let copy = plan.clone();
    let repeat = tokio::task::spawn_blocking(move || prepare(&copy))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repeat.plan_digest, manifest.plan_digest);
    // A restored image can produce an independent full working tree with original refs/bytes.
    let checkout = root.path().join("restored-checkout");
    let result = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&manifest.repositories[0].path)
        .arg(&checkout)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        fs::read_to_string(checkout.join("tracked")).unwrap(),
        "original bytes"
    );
    assert_eq!(run(&checkout, &["rev-parse", "HEAD"]), commit);
    let mut failed = plan.clone();
    failed.build_id = "failed-rebuild".into();
    failed.status_directory = root.path().join("failed-status");
    failed.repositories[0].repository_url = format!("http://{address}/owner/missing.git");
    failed.repositories[0].repository_identity = "127.0.0.1/owner/missing".into();
    token(&failed);
    let copy = failed.clone();
    assert!(
        tokio::task::spawn_blocking(move || prepare(&copy))
            .await
            .unwrap()
            .is_err()
    );
    assert!(!failed.status_directory.join("manifest.json").exists());
    assert!(fs::read_dir(&plan.secrets_root).unwrap().next().is_none());
    assert_eq!(
        fs::read_to_string(plan.status_directory.join("manifest.json")).unwrap(),
        output
    );
    assert!(
        fs::read_to_string(failed.status_directory.join("progress.json"))
            .unwrap()
            .contains("failed")
    );
    prepare(&plan).expect("Previous prepared cache must remain usable after a failed rebuild");
    let mut conflicting = plan.clone();
    conflicting.build_id = "different-build".into();
    let result = prepare(&conflicting);
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(plan.status_directory.join("manifest.json")).unwrap(),
        output
    );
    fs::write(
        manifest.repositories[0].path.join("refs/heads/main"),
        "0000000000000000000000000000000000000000\n",
    )
    .unwrap();
    assert!(prepare(&plan).is_err());
    assert_eq!(
        fs::read_to_string(plan.status_directory.join("manifest.json")).unwrap(),
        output
    );
    server.abort();
}
#[test]
fn enrolled_data_credentials_and_untrusted_repository_urls_cannot_become_images() {
    let root = tempfile::tempdir().unwrap();
    let mut plan = plan(root.path(), "https://github.com/owner/repo.git".into());
    plan.repositories[0].repository_identity = "github.com/owner/repo".into();
    token(&plan);
    fs::create_dir_all(plan.home.join(".zuse-data")).unwrap();
    fs::write(plan.home.join(".zuse-data/user-history"), "preserve me").unwrap();
    assert!(prepare(&plan).is_err());
    assert_eq!(
        fs::read_to_string(plan.home.join(".zuse-data/user-history")).unwrap(),
        "preserve me"
    );
    assert!(!plan.status_directory.join("manifest.json").exists());
    for url in [
        "http://github.com/owner/repo.git",
        "https://secret@github.com/owner/repo.git",
        "https://github.com/owner/repo.git?token=secret",
        "file:///tmp/repo",
        "https://github.com/owner/../repo.git",
    ] {
        let mut repo = plan.repositories[0].clone();
        repo.repository_url = url.into();
        assert!(validate_repo(&repo).is_err());
    }
}
