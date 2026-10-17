//! Load the existing Zuse delegation tools into this Pi process only.
use crate::{HarnessError, process::Command, scratch::ScratchDir};

pub(super) fn configure(
    cmd: &mut Command,
    config: &zuse_protocol::McpServer,
) -> Result<ScratchDir, HarnessError> {
    let scratch = ScratchDir::new("pi-mcp")?;
    let extension = scratch.path().join("zuse-mcp.mjs");
    std::fs::write(&extension, include_str!("mcp.mjs"))?;
    cmd.arg("--extension").arg(extension).env(
        "ZUSE_PI_MCP",
        serde_json::to_string(config).expect("serializable MCP config"),
    );
    Ok(scratch)
}
