//! The broker-backed image currently admits Claude launches. This catalogue never probes a CLI.
use zuse_protocol::{
    HarnessId,
    cloud_catalog::{CloudAgentCatalog, CloudAgentModels},
};

pub fn launch_catalog() -> CloudAgentCatalog {
    CloudAgentCatalog {
        providers: vec![CloudAgentModels {
            id: HarnessId::ClaudeCode,
            name: "Claude Code".into(),
            default_model: "claude-sonnet-5".into(),
            models: zuse_providers::claude::catalog::static_models(),
        }],
    }
}
