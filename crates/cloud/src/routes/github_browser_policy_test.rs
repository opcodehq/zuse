//! Real Chrome form navigation against the actual HTML response policy.
use super::*;
#[tokio::test]
#[ignore = "requires installed Chrome and agent-browser"]
async fn browser_form_preserves_origin_without_referring_callback_secrets() {
    use axum::{
        Router,
        routing::{get, post},
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let html = "<form method=post action='/select'><button>Use installation</button></form>";
    let app = Router::new()
        .route(
            "/broken",
            get(move || async move {
                let mut response = github_browser_response(html.into());
                response.headers_mut().insert(
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("no-referrer"),
                );
                response
            }),
        )
        .route(
            "/fixed",
            get(move || async move { github_browser_response(html.into()) }),
        )
        .route(
            "/select",
            post(move |headers: HeaderMap| {
                let tx = tx.clone();
                async move {
                    let value = |name| {
                        headers
                            .get(name)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_owned()
                    };
                    tx.send((value(header::ORIGIN), value(header::REFERER)))
                        .unwrap();
                    "Submitted"
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = format!("zuse-gh-policy-{}", uuid::Uuid::new_v4().simple());
    async fn browser(session: &str, args: &[&str]) {
        let output = tokio::process::Command::new("agent-browser")
            .args(["--session", session])
            .args(args)
            .env("AGENT_BROWSER_EXECUTABLE_PATH", "/usr/bin/google-chrome")
            .output()
            .await
            .expect("agent-browser installed");
        assert!(
            output.status.success(),
            "browser fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (path, expected) in [("broken", "null"), ("fixed", origin.as_str())] {
        browser(
            &session,
            &[
                "open",
                &format!("{origin}/{path}?code=synthetic-secret&state=synthetic-state"),
            ],
        )
        .await;
        browser(&session, &["snapshot", "-i"]).await;
        browser(&session, &["click", "button"]).await;
        let (actual, referer) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
        if path == "fixed" {
            assert_eq!(referer, format!("{origin}/"));
        }
    }
    browser(&session, &["close"]).await;
    server.abort();
}
