use crate::{Error, PairId, PairSecret};
use snow::{Builder, HandshakeState, TransportState};
use zeroize::Zeroizing;

const PATTERN: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const PURPOSE: &[u8] = b"zuse.paired-rpc.v1";
const MAX_HANDSHAKE: usize = 512;
pub const MAX_RECORD_BYTES: usize = 65_535;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAYLOAD: usize = MAX_RECORD_BYTES - 16 - 5;

fn handshake(id: &PairId, secret: &PairSecret, client: bool) -> Result<HandshakeState, Error> {
    let prologue = [PURPOSE, b"\0", id.as_str().as_bytes()].concat();
    let builder = Builder::new(PATTERN.parse().map_err(|_| Error::Authentication)?)
        .psk(0, secret.bytes())
        .map_err(|_| Error::Authentication)?
        .prologue(&prologue)
        .map_err(|_| Error::Authentication)?;
    if client {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(|_| Error::Authentication)
}
fn write(state: &mut HandshakeState) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0u8; MAX_HANDSHAKE];
    let n = state
        .write_message(PURPOSE, &mut bytes)
        .map_err(|_| Error::Authentication)?;
    bytes.truncate(n);
    Ok(bytes)
}
fn read(state: &mut HandshakeState, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() > MAX_HANDSHAKE {
        return Err(Error::Limit);
    }
    let mut payload = Zeroizing::new(vec![0u8; MAX_HANDSHAKE]);
    let n = state
        .read_message(bytes, &mut payload)
        .map_err(|_| Error::Authentication)?;
    if &payload[..n] != PURPOSE {
        return Err(Error::Authentication);
    }
    Ok(())
}

pub struct ClientHandshake(HandshakeState);
pub fn initiate(id: &PairId, secret: &PairSecret) -> Result<(ClientHandshake, Vec<u8>), Error> {
    let mut state = handshake(id, secret, true)?;
    let first = write(&mut state)?;
    Ok((ClientHandshake(state), first))
}
impl ClientHandshake {
    pub fn finish(mut self, reply: &[u8]) -> Result<SecureChannel, Error> {
        read(&mut self.0, reply)?;
        Ok(SecureChannel::new(
            self.0
                .into_transport_mode()
                .map_err(|_| Error::Authentication)?,
        ))
    }
}
pub fn accept(
    id: &PairId,
    secret: &PairSecret,
    first: &[u8],
) -> Result<(SecureChannel, Vec<u8>), Error> {
    let mut state = handshake(id, secret, false)?;
    read(&mut state, first)?;
    let reply = write(&mut state)?;
    Ok((
        SecureChannel::new(
            state
                .into_transport_mode()
                .map_err(|_| Error::Authentication)?,
        ),
        reply,
    ))
}

struct Pending {
    total: usize,
    bytes: Vec<u8>,
}
pub struct SecureChannel {
    state: TransportState,
    pending: Option<Pending>,
    closed: bool,
}
impl SecureChannel {
    fn new(state: TransportState) -> Self {
        Self {
            state,
            pending: None,
            closed: false,
        }
    }
    /// One application message becomes ordered, individually authenticated records.
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if message.len() > MAX_MESSAGE_BYTES {
            return Err(Error::Limit);
        }
        let mut records = vec![];
        let mut offset = 0;
        loop {
            let end = (offset + MAX_PAYLOAD).min(message.len());
            let mut plain = Zeroizing::new(Vec::with_capacity(end - offset + 5));
            plain.push(u8::from(offset == 0));
            if offset == 0 {
                plain.extend_from_slice(&(message.len() as u32).to_be_bytes());
            }
            plain.extend_from_slice(&message[offset..end]);
            let mut record = vec![0u8; plain.len() + 16];
            match self.state.write_message(&plain, &mut record) {
                Ok(n) => record.truncate(n),
                Err(_) => {
                    self.close();
                    return Err(Error::Closed);
                }
            }
            records.push(record);
            if end == message.len() {
                return Ok(records);
            }
            offset = end;
        }
    }
    /// Any authentication, order or framing error permanently closes this session.
    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.open_record(record);
        if result.is_err() {
            self.close();
        }
        result
    }
    fn open_record(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        if record.len() > MAX_RECORD_BYTES {
            return Err(Error::Limit);
        }
        let mut plain = Zeroizing::new(vec![0u8; record.len()]);
        let n = self
            .state
            .read_message(record, &mut plain)
            .map_err(|_| Error::Authentication)?;
        let plain = &plain[..n];
        let payload = match plain.first() {
            Some(1) if self.pending.is_none() && plain.len() >= 5 => {
                let total = u32::from_be_bytes(plain[1..5].try_into().map_err(|_| Error::Framing)?)
                    as usize;
                if total > MAX_MESSAGE_BYTES {
                    return Err(Error::Limit);
                }
                self.pending = Some(Pending {
                    total,
                    bytes: Vec::with_capacity(total.min(MAX_PAYLOAD)),
                });
                &plain[5..]
            }
            Some(0) if self.pending.is_some() => &plain[1..],
            _ => return Err(Error::Framing),
        };
        let pending = self.pending.as_mut().ok_or(Error::Framing)?;
        if pending.bytes.len().saturating_add(payload.len()) > pending.total
            || (payload.is_empty() && pending.total != 0)
        {
            return Err(Error::Framing);
        }
        let needed = pending.bytes.len() + payload.len();
        if needed > pending.bytes.capacity() {
            let capacity = pending
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(pending.total);
            pending.bytes.reserve_exact(capacity - pending.bytes.len());
        }
        pending.bytes.extend_from_slice(payload);
        if pending.bytes.len() == pending.total {
            return Ok(self.pending.take().map(|p| p.bytes));
        }
        Ok(None)
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.pending = None;
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_invalid_lengths_and_interleaved_messages_close_the_channel() {
        for attack in 0..4 {
            let secret = PairSecret::generate().unwrap();
            let id = PairId::generate().unwrap();
            let (client, first) = initiate(&id, &secret).unwrap();
            let (mut host, reply) = accept(&id, &secret, &first).unwrap();
            let mut client = client.finish(&reply).unwrap();
            let plain = match attack {
                0 => [
                    vec![1],
                    ((MAX_MESSAGE_BYTES + 1) as u32).to_be_bytes().to_vec(),
                ]
                .concat(),
                1 => vec![0, 7],
                2 => {
                    let first = [1, 0, 0, 0, 2, 7];
                    let mut record = vec![0; first.len() + 16];
                    let n = client.state.write_message(&first, &mut record).unwrap();
                    assert!(host.open(&record[..n]).unwrap().is_none());
                    vec![1, 0, 0, 0, 1, 7]
                }
                _ => vec![1, 0, 0, 0, 1, 7, 8],
            };
            let mut record = vec![0; plain.len() + 16];
            let n = client.state.write_message(&plain, &mut record).unwrap();
            record.truncate(n);
            let error = host.open(&record).unwrap_err();
            assert!(matches!(
                (&error, attack),
                (Error::Limit, 0) | (Error::Framing, 1..=3)
            ));
            assert!(host.is_closed());
        }
    }
}
