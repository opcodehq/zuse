//! Encrypted transport controls, disjoint from UTF-8 JSON RPC payloads.
//! A fresh challenge must reach the execution host; WebSocket pongs do not count.
use crate::Error;
const PREFIX: &[u8] = b"\0zuse-paired-heartbeat-v1:";
const NONCE_BYTES: usize = 16;
pub const INTERVAL_MS: u64 = 20_000;
pub const TIMEOUT_MS: u64 = 60_000;
pub const INITIAL_TIMEOUT_MS: u64 = 10_000;

pub enum Action {
    Send(Vec<u8>),
    Wait(u64),
    Expired,
}
pub struct Heartbeat {
    pending: Option<([u8; NONCE_BYTES], u64)>,
    next_at: u64,
    confirmed: bool,
}
fn frame(kind: u8, nonce: &[u8; NONCE_BYTES]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(PREFIX.len() + 1 + NONCE_BYTES);
    payload.extend_from_slice(PREFIX);
    payload.push(kind);
    payload.extend_from_slice(nonce);
    payload
}
fn control(payload: &[u8], kind: u8) -> Result<Option<[u8; NONCE_BYTES]>, Error> {
    if !payload.starts_with(PREFIX) {
        return Ok(None);
    }
    if payload.len() != PREFIX.len() + 1 + NONCE_BYTES || payload[PREFIX.len()] != kind {
        return Err(Error::Heartbeat);
    }
    Ok(Some(
        payload[PREFIX.len() + 1..]
            .try_into()
            .map_err(|_| Error::Heartbeat)?,
    ))
}
/// Called only after successful Noise decryption and live pairing authorization.
pub fn response(payload: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    Ok(control(payload, 0)?.map(|nonce| frame(1, &nonce)))
}
impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}
impl Heartbeat {
    pub fn new() -> Self {
        Self {
            pending: None,
            next_at: 0,
            confirmed: false,
        }
    }
    /// Caller supplies elapsed monotonic milliseconds, preserved across cancelled waits.
    pub fn poll(&mut self, now: u64) -> Result<Action, Error> {
        if let Some((_, deadline)) = self.pending {
            return Ok(if now >= deadline {
                Action::Expired
            } else {
                Action::Wait(deadline - now)
            });
        }
        if now < self.next_at {
            return Ok(Action::Wait(self.next_at - now));
        }
        let mut nonce = [0; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
        self.pending = Some((
            nonce,
            now.saturating_add(if self.confirmed {
                TIMEOUT_MS
            } else {
                INITIAL_TIMEOUT_MS
            }),
        ));
        Ok(Action::Send(frame(0, &nonce)))
    }
    /// Only a matching, timely encrypted reply advances responsiveness.
    /// Ordinary application traffic and link pongs cannot acknowledge a probe.
    pub fn consume(&mut self, payload: &[u8], now: u64) -> Result<bool, Error> {
        let Some(nonce) = control(payload, 1)? else {
            return Ok(false);
        };
        let Some((expected, deadline)) = self.pending else {
            return Err(Error::Heartbeat);
        };
        if nonce != expected || now >= deadline {
            return Err(Error::Heartbeat);
        }
        self.pending = None;
        self.confirmed = true;
        self.next_at = now.saturating_add(INTERVAL_MS);
        Ok(true)
    }
}
