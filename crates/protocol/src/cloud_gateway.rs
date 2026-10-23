//! Opaque workspace gateway framing, compatible with retained v2/v3 runtimes.
pub const LEGACY_PROTOCOL: &str = "zuse-workspace-v1";
pub const PROTOCOL: &str = "zuse-workspace-v2";
pub const PENDING_PROTOCOL: &str = "zuse-workspace-v3";
/// Inner RPC is negotiated separately from the retained gateway's outer framing.
pub const RUST_PROTOCOL: &str = "zuse-workspace-rust-v1";
pub fn supports_availability(protocol: &str) -> bool {
    protocol == PENDING_PROTOCOL || protocol == RUST_PROTOCOL
}
pub const RUST_RPC: &str = "zuse-rpc-v1";
pub const LEGACY_RPC: &str = "zuse-effect-rpc-v1";
pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
pub const MAX_FRAME: usize = MAX_PAYLOAD + 521;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Client,
    Runtime,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Text(String),
    Binary(Vec<u8>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub direction: Direction,
    pub connection_id: String,
    pub payload: Payload,
}
impl Frame {
    pub fn encode(&self) -> Option<Vec<u8>> {
        let connection = self.connection_id.as_bytes();
        let (encoding, payload): (u8, &[u8]) = match &self.payload {
            Payload::Text(s) => (1, s.as_bytes()),
            Payload::Binary(b) => (2, b),
        };
        if connection.is_empty() || connection.len() > 512 || payload.len() > MAX_PAYLOAD {
            return None;
        }
        let mut out = Vec::with_capacity(9 + connection.len() + payload.len());
        out.extend_from_slice(b"ZUSE");
        out.extend_from_slice(&[
            1,
            match self.direction {
                Direction::Client => 1,
                Direction::Runtime => 2,
            },
            encoding,
        ]);
        out.extend_from_slice(&(connection.len() as u16).to_be_bytes());
        out.extend_from_slice(connection);
        out.extend_from_slice(payload);
        Some(out)
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 9 || &bytes[..4] != b"ZUSE" || bytes[4] != 1 {
            return None;
        }
        let direction = match bytes[5] {
            1 => Direction::Client,
            2 => Direction::Runtime,
            _ => return None,
        };
        let len = u16::from_be_bytes([bytes[7], bytes[8]]) as usize;
        if len == 0 || len > 512 || bytes.len() < 9 + len || bytes.len() - 9 - len > MAX_PAYLOAD {
            return None;
        }
        let connection_id = std::str::from_utf8(&bytes[9..9 + len]).ok()?.to_owned();
        let body = &bytes[9 + len..];
        let payload = match bytes[6] {
            1 => Payload::Text(std::str::from_utf8(body).ok()?.to_owned()),
            2 => Payload::Binary(body.to_vec()),
            _ => return None,
        };
        Some(Self {
            direction,
            connection_id,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_runtime_wire_is_exact_and_payload_remains_opaque() {
        // Original nine-byte header, network-order ID length, UTF-8 ID, opaque bytes.
        let bytes = b"ZUSE\x01\x02\x02\x00\x03c-1\x00\xff\x12";
        let frame = Frame::decode(bytes).unwrap();
        assert_eq!(frame.direction, Direction::Runtime);
        assert_eq!(frame.connection_id, "c-1");
        assert_eq!(frame.payload, Payload::Binary(vec![0, 255, 18]));
        assert_eq!(frame.encode().unwrap(), bytes);
        for bad in [
            &bytes[..8],
            b"ZUSE\x02\x02\x02\x00\x03c-1",
            b"ZUSE\x01\x02\x01\x00\x03c-1\xff",
            b"ZUSE\x01\x02\x02\x02\x01",
        ] {
            assert!(Frame::decode(bad).is_none());
        }
    }
}
