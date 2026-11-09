//! A paired session that outlives any one socket.
//!
//! Each socket is still its own Noise channel with fresh keys. Above that, both
//! sides count the application messages they have delivered and keep what the
//! peer has not acknowledged, so a replacement socket resumes exactly where
//! the old one stopped: nothing is lost, repeated or executed twice.
use crate::{Error, MAX_MESSAGE_BYTES, routes::Route};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Unknown to older hosts, which answer with an ordinary RPC error.
pub const ATTACH_METHOD: &str = "PairedAttach";
pub const ATTACH_ID: u64 = 9_007_199_254_740_991;
pub const CAP_LZ4: &str = "lz4";
/// How long a host keeps a session with no socket attached.
pub const HOST_HOLD_MS: u64 = 60_000;
/// Clients stop trying before the host gives up, leaving time for one attach.
pub const CLIENT_RESUME_WINDOW_MS: u64 = 50_000;
/// Sending pauses here; a single message may overshoot by its own size.
pub const MAX_UNACKED_BYTES: usize = 8 * 1024 * 1024;
pub const ACK_INTERVAL_MS: u64 = 1_000;
const ACK_EVERY_MESSAGES: u64 = 16;
const ACK_EVERY_BYTES: usize = 256 * 1024;
const COMPRESS_MIN_BYTES: usize = 1024;

const PREFIX: &[u8] = b"\0zuse-paired-session-v1:";
const KIND_ACK: u8 = 0;
const KIND_ROUTES: u8 = 1;
const KIND_END: u8 = 2;
const COMPRESSED: u8 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachRequest {
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub received: u64,
    #[serde(default)]
    pub caps: Vec<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachReply {
    pub session: String,
    pub resumed: bool,
    pub received: u64,
    #[serde(default)]
    pub caps: Vec<String>,
    #[serde(default)]
    pub engine_info: Option<serde_json::Value>,
    #[serde(default)]
    pub routes: Vec<Route>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Control {
    Ack(u64),
    Routes(Vec<Route>),
    /// The host ended this session; resuming cannot succeed.
    End,
}
pub fn ack_frame(received: u64) -> Vec<u8> {
    let mut frame = control_header(KIND_ACK);
    frame.extend_from_slice(&received.to_be_bytes());
    frame
}
pub fn routes_frame(routes: &[Route]) -> Result<Vec<u8>, Error> {
    let mut frame = control_header(KIND_ROUTES);
    frame.extend(serde_json::to_vec(routes).map_err(|_| Error::Framing)?);
    Ok(frame)
}
pub fn end_frame() -> Vec<u8> {
    control_header(KIND_END)
}
fn control_header(kind: u8) -> Vec<u8> {
    let mut frame = Vec::with_capacity(PREFIX.len() + 9);
    frame.extend_from_slice(PREFIX);
    frame.push(kind);
    frame
}
pub fn parse_control(payload: &[u8]) -> Result<Option<Control>, Error> {
    let Some(body) = payload.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    match body.split_first() {
        Some((&KIND_ACK, count)) => Ok(Some(Control::Ack(u64::from_be_bytes(
            count.try_into().map_err(|_| Error::Framing)?,
        )))),
        Some((&KIND_ROUTES, json)) => Ok(Some(Control::Routes(
            serde_json::from_slice(json).map_err(|_| Error::Framing)?,
        ))),
        Some((&KIND_END, [])) => Ok(Some(Control::End)),
        _ => Err(Error::Framing),
    }
}

/// Ciphertext cannot be compressed, so this runs before `seal`. Application
/// JSON never starts with the marker byte, and each message stands alone so
/// one message's contents cannot influence another's size.
pub fn encode_message(message: &[u8], compress: bool) -> Vec<u8> {
    if compress && message.len() >= COMPRESS_MIN_BYTES {
        let body = lz4_flex::block::compress_prepend_size(message);
        if body.len() + 1 < message.len() {
            let mut encoded = Vec::with_capacity(body.len() + 1);
            encoded.push(COMPRESSED);
            encoded.extend_from_slice(&body);
            return encoded;
        }
    }
    message.to_vec()
}
pub fn decode_message(encoded: Vec<u8>) -> Result<Vec<u8>, Error> {
    if encoded.first() != Some(&COMPRESSED) {
        return Ok(encoded);
    }
    let body = &encoded[1..];
    let size: [u8; 4] = body
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Error::Framing)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > MAX_MESSAGE_BYTES {
        return Err(Error::Limit);
    }
    lz4_flex::block::decompress(&body[4..], size).map_err(|_| Error::Framing)
}

/// One direction's retransmission buffer plus the other's delivery count.
#[derive(Default)]
pub struct Resume {
    sent: VecDeque<Vec<u8>>,
    base: u64,
    buffered: usize,
    received: u64,
    acknowledged: u64,
    bytes_since_ack: usize,
}
impl Resume {
    pub fn can_send(&self) -> bool {
        self.buffered < MAX_UNACKED_BYTES
    }
    pub fn buffered(&self) -> usize {
        self.buffered
    }
    pub fn received(&self) -> u64 {
        self.received
    }
    fn next_seq(&self) -> u64 {
        self.base + self.sent.len() as u64
    }
    pub fn record_sent(&mut self, wire: Vec<u8>) {
        self.buffered += wire.len();
        self.sent.push_back(wire);
    }
    /// The peer's count may only move within what was actually sent.
    pub fn acknowledge(&mut self, received: u64) -> Result<(), Error> {
        if received < self.base || received > self.next_seq() {
            return Err(Error::Framing);
        }
        while self.base < received {
            if let Some(wire) = self.sent.pop_front() {
                self.buffered -= wire.len();
            }
            self.base += 1;
        }
        Ok(())
    }
    /// Count one delivered message; returns an acknowledgement when one is due.
    pub fn record_received(&mut self, bytes: usize) -> Option<Vec<u8>> {
        self.received += 1;
        self.bytes_since_ack += bytes;
        (self.received - self.acknowledged >= ACK_EVERY_MESSAGES
            || self.bytes_since_ack >= ACK_EVERY_BYTES)
            .then(|| self.take_ack())
    }
    /// An acknowledgement for the idle timer, when anything is outstanding.
    pub fn pending_ack(&mut self) -> Option<Vec<u8>> {
        (self.received > self.acknowledged).then(|| self.take_ack())
    }
    fn take_ack(&mut self) -> Vec<u8> {
        self.acknowledged = self.received;
        self.bytes_since_ack = 0;
        ack_frame(self.received)
    }
    /// Everything the peer has not seen, in order, after it reports its count.
    pub fn rewind(&mut self, peer_received: u64) -> Result<Vec<Vec<u8>>, Error> {
        self.acknowledge(peer_received)?;
        self.acknowledged = self.received;
        Ok(self.sent.iter().cloned().collect())
    }
}

pub fn generate_session_id() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::RouteKind;

    #[test]
    fn a_replacement_socket_gets_exactly_the_missing_messages() {
        let mut host = Resume::default();
        for index in 0..5u8 {
            host.record_sent(vec![index]);
        }
        host.acknowledge(1).unwrap();
        assert_eq!(host.rewind(3).unwrap(), vec![vec![3], vec![4]]);
        assert_eq!(host.buffered(), 2);
        assert_eq!(host.rewind(3).unwrap().len(), 2);
    }

    #[test]
    fn counts_outside_what_was_sent_are_refused() {
        let mut host = Resume::default();
        host.record_sent(vec![0]);
        host.acknowledge(1).unwrap();
        assert!(host.acknowledge(0).is_err());
        assert!(host.acknowledge(2).is_err());
        assert!(host.rewind(9).is_err());
    }

    #[test]
    fn sending_pauses_at_the_unacknowledged_limit() {
        let mut host = Resume::default();
        host.record_sent(vec![0; MAX_UNACKED_BYTES]);
        assert!(!host.can_send());
        host.acknowledge(1).unwrap();
        assert!(host.can_send());
    }

    #[test]
    fn acknowledgements_batch_by_count_and_bytes() {
        let mut peer = Resume::default();
        for _ in 0..15 {
            assert!(peer.record_received(1).is_none());
        }
        assert_eq!(peer.record_received(1), Some(ack_frame(16)));
        assert_eq!(peer.pending_ack(), None);
        assert_eq!(peer.record_received(ACK_EVERY_BYTES), Some(ack_frame(17)));
        peer.record_received(1);
        assert_eq!(peer.pending_ack(), Some(ack_frame(18)));
    }

    #[test]
    fn controls_are_disjoint_from_json_and_validated() {
        assert_eq!(parse_control(b"{\"id\":1}").unwrap(), None);
        assert_eq!(parse_control(&ack_frame(7)).unwrap(), Some(Control::Ack(7)));
        let routes = vec![Route {
            kind: RouteKind::Lan,
            origin: "http://10.0.0.2:1".into(),
        }];
        assert_eq!(
            parse_control(&routes_frame(&routes).unwrap()).unwrap(),
            Some(Control::Routes(routes))
        );
        assert!(parse_control(&control_header(KIND_ACK)).is_err());
        assert!(parse_control(&control_header(9)).is_err());
    }

    #[test]
    fn compression_round_trips_and_skips_small_or_dense_messages() {
        let text = br#"{"id":1,"item":"hello"}"#.repeat(200);
        let encoded = encode_message(&text, true);
        assert!(encoded.len() < text.len() / 4);
        assert_eq!(decode_message(encoded).unwrap(), text);
        assert_eq!(encode_message(b"{}", true), b"{}");
        assert_eq!(encode_message(&text, false), text);
        assert_eq!(decode_message(text.clone()).unwrap(), text);
    }

    #[test]
    fn hostile_compressed_sizes_are_bounded() {
        let mut bomb = vec![COMPRESSED];
        bomb.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(decode_message(bomb), Err(Error::Limit)));
        assert!(decode_message(vec![COMPRESSED, 1]).is_err());
        let mut lying = vec![COMPRESSED];
        lying.extend_from_slice(&64u32.to_le_bytes());
        lying.extend_from_slice(&[0xff; 8]);
        assert!(decode_message(lying).is_err());
    }
}
