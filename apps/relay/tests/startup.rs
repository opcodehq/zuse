#![cfg(unix)]
//! Verify the packaged command serves browser assets without starting an engine.
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
#[tokio::test]
async fn relay_command_serves_assets_requires_host_auth_and_drains_on_sigterm() {
    let root = tempfile::tempdir().unwrap();
    let token = root.path().join("routing-token");
    std::fs::write(&token, "f".repeat(64)).unwrap();
    std::fs::write(
        root.path().join("index.html"),
        "<title>Zuse relay fixture</title>",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_zuse-relay"))
        .args(["--listen", "127.0.0.1:0", "--host-token-file"])
        .arg(&token)
        .arg("--assets")
        .arg(root.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let address = line
        .trim()
        .strip_prefix("Zuse relay listening on ")
        .unwrap();
    let origin = format!("http://{address}");
    let http = reqwest::Client::new();
    assert!(
        http.get(format!("{origin}/health"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let page = http.get(&origin).send().await.unwrap();
    assert_eq!(
        page.headers()["cross-origin-embedder-policy"],
        "require-corp"
    );
    assert!(page.text().await.unwrap().contains("Zuse relay fixture"));
    assert_eq!(
        http.post(format!("{origin}/api/session"))
            .send()
            .await
            .unwrap()
            .status(),
        405
    );
    let unauthorized = tokio_tungstenite::connect_async(format!(
        "ws://{address}/api/paired-host/{}",
        "0".repeat(32)
    ))
    .await;
    assert!(
        matches!(unauthorized, Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 401)
    );
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().unwrap().to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(http.get(format!("{origin}/health")).send().await.is_err());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&token).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let rejected = Command::new(env!("CARGO_BIN_EXE_zuse-relay"))
        .args(["--listen", "0.0.0.0:0", "--host-token-file"])
        .arg(&token)
        .arg("--assets")
        .arg(root.path())
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("requires direct TLS"));
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains(&"f".repeat(64)));
}
