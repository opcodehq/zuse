//! zuse-protocol — wire types shared by engine, UI, and RPC.
//!
//! Ported from zuse's `packages/control/src/wire.ts` + `packages/harness/src/types.ts`.
//! Context occupancy is replicated per chat; billing `Usage` remains a harness passthrough.

pub mod agent;
pub mod attachment_mentions;
pub mod change_request_assessment;
pub mod chat_config;
pub mod chat_mentions;
pub mod cloud;
pub mod custom_acp;
pub mod device_paths;
pub mod entities;
pub mod file_mentions;
pub mod git_workspace;
pub mod input_answers;
pub mod invocation;
pub mod managed_connection;
pub mod methods;
pub mod motion;
pub mod plugins;
pub mod presentation;
pub mod preview;
pub mod projectless;
pub mod recovery;
pub mod sidebar_pins;
pub mod view;
pub mod visual_session;
pub mod voice;
pub mod workspace;

pub use agent::*;
pub use chat_config::ChatConfigPatch;
pub use entities::*;
pub use preview::*;
pub use sidebar_pins::*;
pub use workspace::*;

/// Parse "0.2.12" (tolerating a `-suffix`/`+build` tail on the last part)
/// into a comparable triple — the fleet feature-gate primitive (device rows
/// stamp `Device::version` at boot). `None` for anything that doesn't lead
/// with three dotted integers, and gates treat `None` as "too old".
pub fn version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().splitn(3, '.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    let patch: u64 = patch
        .split(['-', '+'])
        .next()
        .unwrap_or(patch)
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

pub mod message_attachments;

pub mod browser_drafts;
pub mod browser_link;

pub mod provider_setup;

pub mod completion_sound;
pub mod notifications;

pub mod plugin_setup;
pub mod plugin_state;
