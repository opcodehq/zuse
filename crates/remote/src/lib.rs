//! Pair-authorized encrypted RPC messages shared by native and browser clients.
//!
//! Noise provides the handshake, key exchange, encryption and nonce sequencing.
//! This module only bounds and reassembles application messages into Noise records.
mod channel;
mod invitation;
pub use channel::{
    ClientHandshake, MAX_MESSAGE_BYTES, MAX_RECORD_BYTES, SecureChannel, accept, initiate,
};
pub use invitation::{Invitation, PairId, PairSecret};

#[derive(Debug, thiserror::Error)]
pub enum Error {
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
