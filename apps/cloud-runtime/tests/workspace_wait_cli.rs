//! The instruction's real command must wait without enrolling or acquiring the runtime lock.
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("workspace-preparation.json"));
        let _ = std::fs::remove_dir(&self.0);
    }
}
#[test]
fn workspace_wait_cli_is_local_fenced_and_usable_while_the_runtime_owns_storage() {
    let root = Directory(std::env::temp_dir().join(format!(
            "zuse-workspace-wait-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&root.0).unwrap();
    for (state, generation, succeeds, error) in [
        ("ready", "1", true, ""),
        ("failed", "1", false, "workspace_preparation_failed"),
        ("ready", "2", false, "workspace_preparation_fenced"),
        ("cancelled", "1", false, "workspace_preparation_cancelled"),
    ] {
        std::fs::write(
            root.0.join("workspace-preparation.json"),
            format!(r#"{{"workspace":"task","generation":1,"disk":"disk","state":"{state}"}}"#),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_zuse-cloud-runtime"))
            .arg("--data-dir")
            .arg(&root.0)
            .args([
                "--workspace-id",
                "task",
                "--generation",
                generation,
                "--storage-incarnation-id",
                "disk",
                "workspace-wait",
                "--timeout-seconds",
                "1",
            ])
            .env_clear()
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !succeeds {
            assert!(String::from_utf8_lossy(&output.stderr).contains(error));
        }
        assert!(!root.0.join("cloud-runtime-identity.json").exists());
        assert!(!root.0.join(".workspace-runtime.lock").exists());
    }
}
