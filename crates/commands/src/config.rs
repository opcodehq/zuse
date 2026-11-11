//! Engine configuration from the environment, shared by the window,
//! `headless`, `login`, `logout` and `status`, so the auth commands always
//! act on the exact session the engine will load.
use zuse_engine::{EngineConfig, HarnessId};

/// Optional workspace/device/relay edge; local development remains the default.
/// `ZUSE_EDGE_URL` overrides (local dev / self-hosting).
pub fn edge_url_from_env() -> String {
    std::env::var("ZUSE_EDGE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| zuse_update::LOCAL_EDGE_URL.into())
}

/// Optional workspace/relay auth: explicit public client wins (empty = dev mode).
/// Without one, workspace sync remains local-only. General account sign-in uses
/// engine::account's browser sign-in, whose client follows the API origin.
pub fn workos_client_id_from_env() -> Option<String> {
    std::env::var("ZUSE_WORKOS_CLIENT_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

pub fn engine_config(ipc_port: u16) -> EngineConfig {
    EngineConfig {
        data_dir: crate::paths::data_dir(),
        edge_url: edge_url_from_env(),
        ipc_port,
        default_harness: harness_from_env(),
        // WorkOS mode: the signed-in session's org wins; ZUSE_ORG_ID (dev
        // default "dev-org") scopes the workspace room otherwise.
        org_id: std::env::var("ZUSE_ORG_ID").ok(),
        // Optional workspace identity; General cloud-account auth is separate.
        workos_client_id: workos_client_id_from_env(),
        // Dev-mode bearer (no WorkOS): an explicit token enables sync.
        edge_token: std::env::var("ZUSE_EDGE_TOKEN").ok(),
    }
}

/// `ZUSE_HARNESS` (kebab-case id) picks the default harness for chats without a
/// config row — `mock` powers the e2e smoke; default `claude-code`.
fn harness_from_env() -> HarnessId {
    match std::env::var("ZUSE_HARNESS").as_deref().map(str::trim) {
        Ok("mock") => HarnessId::Mock,
        Ok("codex") => HarnessId::Codex,
        Ok("cursor") => HarnessId::Cursor,
        Ok("devin") => HarnessId::Devin,
        Ok("grok") => HarnessId::Grok,
        Ok("hermes") => HarnessId::Hermes,
        Ok("pi") => HarnessId::Pi,
        Ok("muse") => HarnessId::Muse,
        Ok("antigravity") => HarnessId::Antigravity,
        _ => HarnessId::ClaudeCode,
    }
}
