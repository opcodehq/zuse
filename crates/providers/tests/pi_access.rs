//! Exercise the shipped guard through its public event handlers.
#[tokio::test]
async fn access_extension_enforces_tools_shells_concurrent_answers_and_cancellation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("node")
            .arg(root.join("tests/fixtures/pi-access-test.mjs"))
            .arg(root.join("src/pi/access.mjs"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("access extension must settle")
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
