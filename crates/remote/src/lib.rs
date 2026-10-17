//! Pair-authorized encrypted RPC messages shared by native and browser clients.
//!
//! Noise provides the handshake, key exchange, encryption and nonce sequencing.
//! This module only bounds and reassembles application messages into Noise records.
mod channel;
mod invitation;
mod target;
pub use channel::{
    ClientHandshake, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, SecureChannel, accept, initiate,
};
pub use invitation::{Invitation, PairId, PairSecret};
pub use target::PairedTarget;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Use an HTTPS engine origin, or HTTP on localhost")]
    Endpoint,
    #[error("Invalid pairing invitation")]
    Invitation,
    #[error("Secure randomness is unavailable")]
    Random,
    #[error("Paired connection authentication failed")]
    Authentication,
    #[error("Paired message exceeds the supported size")]
    Limit,
    #[error("Invalid paired message framing")]
    Framing,
    #[error("Paired connection is closed")]
    Closed,
}

#[cfg(not(target_arch = "wasm32"))]
pub mod authority;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerMetadata {
    pub id: String,
    pub label: String,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub paired_at: Option<u64>,
}
