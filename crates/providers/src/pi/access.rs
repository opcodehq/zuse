//! Install the access guard only in the owned provider process.
use crate::{HarnessError, process::Command, scratch::ScratchDir};
use zuse_protocol::RunRequest;

pub(super) struct Guard {
    pub ready: String,
    _scratch: ScratchDir,
}

pub(super) fn configure(cmd: &mut Command, request: &RunRequest) -> Result<Guard, HarnessError> {
    let scratch = ScratchDir::new("pi-access")?;
    let extension = scratch.path().join("zuse-access.mjs");
    std::fs::write(&extension, include_str!("access.mjs"))?;
    let mode = if request.plan {
        "read-only"
    } else if request.auto_approves_tools() {
        "full"
    } else {
        "ask"
    };
    // Ambient extensions execute arbitrary code at startup and can replace tools
    // or run commands outside tool_call. Restricted runs load only owned bridges.
    if mode != "full" {
        cmd.arg("--no-extensions");
    }
    let ready = format!("zuse-access-{}", uuid::Uuid::new_v4().simple());
    cmd.arg("--extension").arg(extension).env(
        "ZUSE_PI_ACCESS",
        serde_json::json!({"mode":mode,"ready":ready}).to_string(),
    );
    Ok(Guard {
        ready,
        _scratch: scratch,
    })
}
