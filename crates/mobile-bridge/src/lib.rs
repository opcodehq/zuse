//! Mobile bindings reuse the host's authenticated channel and heartbeat protocol.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{CString, c_char},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use zuse_remote::{
    ClientHandshake, PairedTarget, SecureChannel,
    heartbeat::{Action, Heartbeat},
};

struct Session {
    handshake: Option<ClientHandshake>,
    channel: Option<SecureChannel>,
    heartbeat: Heartbeat,
    started: Instant,
    ready: bool,
}
static SESSIONS: OnceLock<Mutex<HashMap<u64, Session>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Start { link: String },
    Finish { handle: u64, record: String },
    Receive { handle: u64, record: String },
    Send { handle: u64, payload: String },
    Poll { handle: u64 },
    Close { handle: u64 },
}
fn encoded(records: Vec<Vec<u8>>) -> Value {
    json!(
        records
            .into_iter()
            .map(|record| B64.encode(record))
            .collect::<Vec<_>>()
    )
}
fn decode(value: &str, limit: usize) -> Result<Vec<u8>, ()> {
    if value.len() > limit.div_ceil(3) * 4 {
        return Err(());
    }
    let bytes = B64.decode(value).map_err(|_| ())?;
    if bytes.len() > limit {
        return Err(());
    }
    Ok(bytes)
}
fn poll(session: &mut Session) -> Result<Value, ()> {
    let now = session.started.elapsed().as_millis() as u64;
    match session.heartbeat.poll(now).map_err(|_| ())? {
        Action::Send(probe) => Ok(
            json!({"records": encoded(session.channel.as_mut().ok_or(())?.seal(&probe).map_err(|_| ())?), "ready": session.ready}),
        ),
        Action::Wait(ms) => Ok(json!({"records": [], "waitMs": ms, "ready": session.ready})),
        Action::Expired => Err(()),
    }
}
/// A bounded local JSON interface; no pairing secret is included in responses or errors.
pub fn execute(input: &str) -> Value {
    let result = (|| -> Result<Value, ()> {
        if input.len() > 24 * 1024 * 1024 {
            return Err(());
        }
        let request: Request = serde_json::from_str(input).map_err(|_| ())?;
        let mut sessions = SESSIONS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| ())?;
        if let Request::Start { link } = request {
            if sessions.len() >= 64 {
                return Err(());
            }
            let target = PairedTarget::parse_link(&link).map_err(|_| ())?;
            let invitation = target.invitation();
            let (handshake, first) =
                zuse_remote::initiate(&invitation.id, &invitation.secret).map_err(|_| ())?;
            let handle = NEXT.fetch_add(1, Ordering::Relaxed);
            if handle == 0 {
                return Err(());
            }
            sessions.insert(
                handle,
                Session {
                    handshake: Some(handshake),
                    channel: None,
                    heartbeat: Heartbeat::new(),
                    started: Instant::now(),
                    ready: false,
                },
            );
            return Ok(
                json!({"handle": handle, "url": target.socket_url(), "records": [B64.encode(first)], "ready": false}),
            );
        }
        let handle = match &request {
            Request::Finish { handle, .. }
            | Request::Receive { handle, .. }
            | Request::Send { handle, .. }
            | Request::Poll { handle }
            | Request::Close { handle } => *handle,
            _ => return Err(()),
        };
        if matches!(request, Request::Close { .. }) {
            sessions.remove(&handle);
            return Ok(json!({"closed": true}));
        }
        // Any error retires the state permanently, including authenticated framing errors.
        let mut session = sessions.remove(&handle).ok_or(())?;
        let output = match request {
            Request::Finish { record, .. } => {
                let reply = decode(&record, 512)?;
                session.channel = Some(
                    session
                        .handshake
                        .take()
                        .ok_or(())?
                        .finish(&reply)
                        .map_err(|_| ())?,
                );
                poll(&mut session)?
            }
            Request::Receive { record, .. } => {
                let record = decode(&record, zuse_remote::MAX_RECORD_BYTES)?;
                match session
                    .channel
                    .as_mut()
                    .ok_or(())?
                    .open(&record)
                    .map_err(|_| ())?
                {
                    None => json!({"ready": session.ready}),
                    Some(payload) => {
                        let now = session.started.elapsed().as_millis() as u64;
                        if session.heartbeat.consume(&payload, now).map_err(|_| ())? {
                            session.ready = true;
                            json!({"ready": true})
                        } else {
                            if !session.ready {
                                return Err(());
                            }
                            json!({"ready": true, "payload": B64.encode(payload)})
                        }
                    }
                }
            }
            Request::Send { payload, .. } => {
                if !session.ready {
                    return Err(());
                }
                let payload = decode(&payload, zuse_remote::MAX_MESSAGE_BYTES)?;
                json!({"ready": true, "records": encoded(session.channel.as_mut().ok_or(())?.seal(&payload).map_err(|_| ())?)})
            }
            Request::Poll { .. } => poll(&mut session)?,
            _ => return Err(()),
        };
        sessions.insert(handle, session);
        Ok(output)
    })();
    match result {
        Ok(value) => value,
        Err(()) => {
            json!({"error": "Paired connection unavailable. Reconnect with a valid invitation."})
        }
    }
}

/// # Safety
/// `input` must point to `len` readable bytes for this call; the returned string must be
/// released exactly once with `zuse_mobile_free`. Input data must not overlap freed storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zuse_mobile_request(input: *const u8, len: usize) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if input.is_null() || len > 24 * 1024 * 1024 {
            return json!({"error":"Invalid bridge request"});
        }
        let bytes = unsafe { std::slice::from_raw_parts(input, len) };
        match std::str::from_utf8(bytes) {
            Ok(input) => execute(input),
            Err(_) => json!({"error":"Invalid bridge request"}),
        }
    })
    .unwrap_or_else(|_| json!({"error":"Native pairing failed"}));
    CString::new(result.to_string())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut())
}
/// # Safety
/// `value` must be null or an unfreed pointer returned by `zuse_mobile_request`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zuse_mobile_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(value: Value) -> Value {
        let output = execute(&value.to_string());
        assert!(output.get("error").is_none(), "bridge operation failed");
        output
    }
    fn record(value: &Value) -> Vec<u8> {
        B64.decode(value["records"][0].as_str().unwrap()).unwrap()
    }
    fn established() -> (u64, SecureChannel) {
        let invitation = zuse_remote::Invitation::generate().unwrap();
        let target = PairedTarget::new("https://host.example", invitation.clone()).unwrap();
        let start = call(json!({"op":"start", "link":target.export_link().unwrap()}));
        assert!(!start.to_string().contains(&invitation.secret.expose()));
        assert_eq!(start["url"], target.socket_url());
        let handle = start["handle"].as_u64().unwrap();
        let (mut host, reply) =
            zuse_remote::accept(&invitation.id, &invitation.secret, &record(&start)).unwrap();
        let finish = call(json!({"op":"finish", "handle":handle, "record":B64.encode(reply)}));
        assert_eq!(finish["ready"], false);
        let challenge = host.open(&record(&finish)).unwrap().unwrap();
        let response = zuse_remote::heartbeat::response(&challenge)
            .unwrap()
            .unwrap();
        let response = host.seal(&response).unwrap();
        let confirmed =
            call(json!({"op":"receive", "handle":handle,"record":B64.encode(&response[0])}));
        assert_eq!(confirmed["ready"], true);
        (handle, host)
    }
    #[test]
    fn real_host_handshake_heartbeat_and_large_records_round_trip() {
        let (handle, mut host) = established();
        let message: Vec<u8> = (0..200_001).map(|i| (i % 251) as u8).collect();
        let sent = call(json!({"op":"send", "handle":handle,"payload":B64.encode(&message)}));
        assert!(sent["records"].as_array().unwrap().len() > 1);
        let mut received = None;
        for value in sent["records"].as_array().unwrap() {
            received = host
                .open(&B64.decode(value.as_str().unwrap()).unwrap())
                .unwrap()
                .or(received);
        }
        assert_eq!(received.unwrap(), message);
        let mut received = None;
        let replies = host.seal(&message).unwrap();
        for reply in &replies {
            let output = call(json!({"op":"receive", "handle":handle,"record":B64.encode(reply)}));
            if let Some(value) = output["payload"].as_str() {
                received = Some(B64.decode(value).unwrap());
            }
        }
        assert_eq!(received.unwrap(), message);
        let replay = execute(
            &json!({"op":"receive", "handle":handle,"record":B64.encode(&replies[0])}).to_string(),
        );
        assert!(replay.get("error").is_some());
        assert!(
            execute(&json!({"op":"poll", "handle":handle}).to_string())
                .get("error")
                .is_some()
        );
        call(json!({"op":"close", "handle":handle}));
    }
    #[test]
    fn messages_cannot_bypass_initial_host_responsiveness_proof() {
        let invitation = zuse_remote::Invitation::generate().unwrap();
        let target = PairedTarget::new("https://host.example", invitation).unwrap();
        let start = call(json!({"op":"start", "link":target.export_link().unwrap()}));
        let handle = start["handle"].as_u64().unwrap();
        assert!(
            execute(&json!({"op":"send", "handle":handle,"payload":"e30="}).to_string())
                .get("error")
                .is_some()
        );
        assert!(
            execute(&json!({"op":"poll", "handle":handle}).to_string())
                .get("error")
                .is_some()
        );
    }
}
