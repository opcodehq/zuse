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
    pending: Option<Pending>,
    next_at: u64,
    confirmed: bool,
    interval_ms: u64,
    timeout_ms: u64,
    expedited_timeout_ms: Option<u64>,
    last_rtt_ms: Option<u32>,
}
struct Pending {
    nonce: [u8; NONCE_BYTES],
    sent_at: u64,
    deadline: u64,
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
            interval_ms: INTERVAL_MS,
            timeout_ms: TIMEOUT_MS,
            expedited_timeout_ms: None,
            last_rtt_ms: None,
        }
    }
    /// How often to check and how long to wait, for this route's latency.
    pub fn set_limits(&mut self, interval_ms: u64, timeout_ms: u64) {
        self.interval_ms = interval_ms;
        self.timeout_ms = timeout_ms;
    }
    /// Check now with a short deadline, because the path may just have changed.
    pub fn expedite(&mut self, now: u64, timeout_ms: u64) {
        match &mut self.pending {
            Some(pending) => pending.deadline = pending.deadline.min(now + timeout_ms),
            None => {
                self.next_at = now;
                self.expedited_timeout_ms = Some(timeout_ms);
            }
        }
    }
    /// Time from the latest challenge to its encrypted reply.
    pub fn last_rtt_ms(&self) -> Option<u32> {
        self.last_rtt_ms
    }
    /// Caller supplies elapsed monotonic milliseconds, preserved across cancelled waits.
    pub fn poll(&mut self, now: u64) -> Result<Action, Error> {
        if let Some(pending) = &self.pending {
            return Ok(if now >= pending.deadline {
                Action::Expired
            } else {
                Action::Wait(pending.deadline - now)
            });
        }
        if now < self.next_at {
            return Ok(Action::Wait(self.next_at - now));
        }
        let mut nonce = [0; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
        let timeout = match self.expedited_timeout_ms.take() {
            Some(timeout) => timeout,
            None if self.confirmed => self.timeout_ms,
            None => INITIAL_TIMEOUT_MS,
        };
        self.pending = Some(Pending {
            nonce,
            sent_at: now,
            deadline: now.saturating_add(timeout),
        });
        Ok(Action::Send(frame(0, &nonce)))
    }
    /// Only a matching, timely encrypted reply advances responsiveness.
    /// Ordinary application traffic and link pongs cannot acknowledge a probe.
    pub fn consume(&mut self, payload: &[u8], now: u64) -> Result<bool, Error> {
        let Some(nonce) = control(payload, 1)? else {
            return Ok(false);
        };
        let Some(pending) = self
            .pending
            .take_if(|p| p.nonce == nonce && now < p.deadline)
        else {
            return Err(Error::Heartbeat);
        };
        self.last_rtt_ms = Some(now.saturating_sub(pending.sent_at).min(u32::MAX as u64) as u32);
        self.confirmed = true;
        self.next_at = now.saturating_add(self.interval_ms);
        Ok(true)
    }
}
