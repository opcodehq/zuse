//! Exercise the executable entry point, rather than calling preparation directly.
use std::process::{Command, Stdio};

#[test]
fn invalid_project_cache_plan_returns_an_error_without_nested_runtime_panic() {
    // A missing file reaches async CLI dispatch (an empty value is rejected by
    // Clap first), without preparing a repository or requiring credentials.
    let missing = std::env::temp_dir().join(format!(
        "zuse-project-cache-plan-{}.missing",
        std::process::id()
    ));
    assert!(!missing.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_zuse-cloud-runtime"))
        .args(["prepare-project-cache", "--plan"])
        .arg(missing)
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .expect("execute the actual guest binary");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 error message");
    assert!(
        stderr.contains("project_cache_preparation_failed"),
        "{stderr}"
    );
    assert!(!stderr.contains("Cannot start a runtime"), "{stderr}");
}
