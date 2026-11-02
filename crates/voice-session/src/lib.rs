//! Shared call presentation; native platforms additionally own the async orchestrator.
mod view;
pub use view::{Caption, SpeakerActivity, VoiceView, orb_state};
#[cfg(not(target_arch = "wasm32"))]
mod orchestrator;
#[cfg(not(target_arch = "wasm32"))]
pub use orchestrator::*;
