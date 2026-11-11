#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, sync::Arc};
use tokio_util::sync::CancellationToken;
use zuse_harness::{PermissionMode, ToolContext};
use zuse_harness_extensions::Registry;

fn configure(profile: &Path, executable: &Path, trusted: bool) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/computer-use-config.py");
    let mut command = Command::new("python3");
    command.arg(script).arg("--cu").arg(executable);
    if trusted {
        command.arg("--trust-desktop");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::create_dir_all(profile.join("harness")).unwrap();
    std::fs::write(profile.join("harness/extensions.json"), output.stdout).unwrap();
}

fn context() -> ToolContext {
    ToolContext {
        agent_id: "cu-compatibility".into(),
        call_id: "discover-only".into(),
        cancel: CancellationToken::new(),
        permission_mode: PermissionMode::Default,
        read_only: false,
    }
}

#[tokio::test]
async fn generated_configuration_requires_explicit_trust() {
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("cu-fixture");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf started > untrusted-started\nexit 99\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    configure(root.path(), &executable, false);
    let registry = Registry::load(root.path()).unwrap();
    let scope = registry
        .scope(root.path().into(), Arc::new(|| "ask".into()))
        .unwrap();
    assert_eq!(scope.servers()["servers"][0]["trusted"], false);
    assert!(!scope.read_only("opcode-cu", "desktop_observe"));
    assert!(scope.tools("opcode-cu", None, context()).await.is_err());
    assert!(!root.path().join("untrusted-started").exists());
}

#[tokio::test]
#[ignore = "explicitly run with CU_EXECUTABLE pointing to a trusted installed CU CLI; discovery only"]
async fn installed_cu_mcp_catalog_is_compatible() {
    let executable = std::env::var_os("CU_EXECUTABLE").expect("Set CU_EXECUTABLE explicitly");
    let root = tempfile::tempdir().unwrap();
    configure(root.path(), Path::new(&executable), true);
    let registry = Registry::load(root.path()).unwrap();
    let scope = registry
        .scope(root.path().into(), Arc::new(|| "ask".into()))
        .unwrap();
    let page = scope.tools("opcode-cu", None, context()).await.unwrap();
    let tools = page["tools"].as_array().unwrap();
    for name in [
        "desktop_status",
        "desktop_observe",
        "desktop_execute",
        "desktop_pause",
    ] {
        assert!(
            tools.iter().any(|tool| tool["name"] == name),
            "Missing {name}"
        );
        assert!(!scope.read_only("opcode-cu", name));
    }
    assert!(!tools.iter().any(|tool| tool["name"] == "desktop_workflow"));
}
