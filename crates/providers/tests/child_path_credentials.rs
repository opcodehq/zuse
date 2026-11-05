//! A task's private command launchers must survive agent PATH composition.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use zeroize::Zeroizing;
use zuse_providers::{credentials::ChildEnvironment, process::Command};

#[tokio::test]
async fn credential_injected_task_path_reaches_the_real_shell_child() {
    let directory = tempfile::tempdir().unwrap();
    let task_bin = directory.path().join("task-bin");
    let vendor_bin = directory.path().join("vendor-bin");
    std::fs::create_dir(&task_bin).unwrap();
    std::fs::create_dir(&vendor_bin).unwrap();
    let launcher = task_bin.join("gh");
    std::fs::write(&launcher, "#!/bin/sh\nprintf 'PRIVATE_TASK_GH\\n'\n").unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();

    let mut child = Command::new("/bin/sh");
    ChildEnvironment::new("PATH", Zeroizing::new(task_bin.to_str().unwrap().to_owned()))
        .apply(&mut child);
    // This is the same order used by Codex, Claude and ACP after authority
    // credentials/workload settings have been applied to the native command.
    zuse_providers::compose_child_path(&mut child, &vendor_bin.join("agent"));
    let output = child.args(["-c", "gh"]).output().await.unwrap();

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "PRIVATE_TASK_GH\n");
}
