//! Mobile bindings reuse the host's authenticated channel and heartbeat protocol.
//!
//! The `link*` operations drive [`zuse_remote::link::Link`]: the apps own their
//! sockets and report what happens to them, and every decision about routes,
//! moving a session and resuming it is made once, here, for both platforms.
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{CString, c_char},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use zuse_remote::{
    ClientHandshake, PairedTarget, Route, SecureChannel,
    heartbeat::{Action, Heartbeat},
    link::{Effects, Link, LinkEnd},
    routes::{SavedRoutes, classify_origin},
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
    Start {
        link: String,
    },
    Finish {
        handle: u64,
        record: String,
    },
    Receive {
        handle: u64,
        record: String,
    },
    Send {
        handle: u64,
        payload: String,
    },
    Poll {
        handle: u64,
    },
    Close {
        handle: u64,
    },
    LinkOpen {
        link: String,
        #[serde(default)]
        store: Option<String>,
    },
    LinkSocketOpened {
        handle: u64,
        socket: u64,
    },
    LinkRecord {
        handle: u64,
        socket: u64,
        record: String,
    },
    LinkSocketClosed {
        handle: u64,
        socket: u64,
    },
    LinkSend {
        handle: u64,
        payload: String,
    },
    LinkTick {
        handle: u64,
    },
    LinkNetworkChanged {
        handle: u64,
    },
    LinkClose {
        handle: u64,
    },
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
        if let Some(output) = link_request(&request) {
            return output;
        }
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

struct LinkEntry {
    link: Link,
    started: Instant,
    store: Option<PathBuf>,
}
impl LinkEntry {
    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}
static LINKS: OnceLock<Mutex<HashMap<u64, LinkEntry>>> = OnceLock::new();

/// An opaque fingerprint of this device's current network, for remembering
/// which route won here. A hash, so saved routes hold no local addresses.
fn network_id() -> String {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| interface.ip())
        .collect();
    addresses.sort();
    let mut hasher = DefaultHasher::new();
    addresses.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn read_saved(store: &Option<PathBuf>) -> Option<SavedRoutes> {
    serde_json::from_slice(&std::fs::read(store.as_ref()?).ok()?).ok()
}

/// Learned routes hold no secrets; failing to save them only costs speed.
fn write_saved(store: &Path, saved: &SavedRoutes) {
    let Ok(bytes) = serde_json::to_vec(saved) else {
        return;
    };
    let temporary = store.with_extension("tmp");
    if let Some(parent) = store.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(temporary, store);
    }
}

fn link_output(entry: Option<&LinkEntry>, effects: Effects) -> Value {
    if let (Some(entry), Some(saved)) = (entry, &effects.saved)
        && let Some(store) = &entry.store
    {
        write_saved(store, saved);
    }
    let mut output = json!({
        "open": effects.open.iter().map(|(socket, url)| json!({"socket": socket, "url": url})).collect::<Vec<_>>(),
        "close": effects.close,
        "write": effects.write.into_iter().map(|(socket, records)| json!({"socket": socket, "records": encoded(records)})).collect::<Vec<_>>(),
        "deliver": encoded(effects.deliver),
        "waitMs": effects.wait_ms,
    });
    if let Some(status) = effects.status {
        output["status"] = json!({
            "state": status.state.as_str(),
            "kind": status.route.as_ref().map(|route| route.kind),
            "label": status.route.as_ref().map(|route| route.kind.label()),
            "rttMs": status.rtt_ms,
            "resumable": status.resumable,
            "compressed": status.compressed,
        });
    }
    if let Some(info) = effects.engine_info {
        output["engineInfo"] = info;
    }
    if effects.connected {
        output["connected"] = json!(true);
    }
    if let Some(ended) = effects.ended {
        output["ended"] = json!(match ended {
            LinkEnd::Unreachable => "unreachable",
            LinkEnd::Authentication => "authentication",
            LinkEnd::Lost => "lost",
            LinkEnd::EndedByHost => "endedByHost",
            LinkEnd::Closed => "closed",
        });
    }
    output
}

fn open_link(link: &str, store: Option<&str>) -> Result<Value, ()> {
    let target = PairedTarget::parse_link(link).map_err(|_| ())?;
    let invitation = target.invitation().clone();
    let store = store
        .map(|directory| Path::new(directory).join(format!("{}.json", invitation.id.as_str())));
    let invited = Route {
        kind: classify_origin(target.origin()),
        origin: target.origin().to_owned(),
    };
    let (link, effects) = Link::open(invited, invitation, read_saved(&store), &network_id(), 0);
    let entry = LinkEntry {
        link,
        started: Instant::now(),
        store,
    };
    let mut links = LINKS.get_or_init(Default::default).lock().map_err(|_| ())?;
    if links.len() >= 64 {
        return Err(());
    }
    let handle = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut output = link_output(Some(&entry), effects);
    output["handle"] = json!(handle);
    links.insert(handle, entry);
    Ok(output)
}

type LinkAct<'a> = &'a mut dyn FnMut(&mut LinkEntry) -> Result<(Effects, Option<bool>), ()>;

/// `None` leaves the request to the single-socket operations.
fn link_request(request: &Request) -> Option<Result<Value, ()>> {
    let run = |handle: u64, act: LinkAct<'_>| -> Result<Value, ()> {
        let mut links = LINKS.get_or_init(Default::default).lock().map_err(|_| ())?;
        let entry = links.get_mut(&handle).ok_or(())?;
        let (effects, accepted) = act(entry)?;
        let ended = effects.ended.is_some();
        let mut output = link_output(Some(entry), effects);
        if let Some(accepted) = accepted {
            output["accepted"] = json!(accepted);
        }
        if ended {
            links.remove(&handle);
        }
        Ok(output)
    };
    Some(match request {
        Request::LinkOpen { link, store } => open_link(link, store.as_deref()),
        Request::LinkSocketOpened { handle, socket } => run(*handle, &mut |e| {
            let now = e.now();
            Ok((e.link.socket_opened(*socket, now), None))
        }),
        Request::LinkRecord {
            handle,
            socket,
            record,
        } => run(*handle, &mut |e| {
            let record = decode(record, zuse_remote::MAX_RECORD_BYTES)?;
            let now = e.now();
            Ok((e.link.record(*socket, record, now), None))
        }),
        Request::LinkSocketClosed { handle, socket } => run(*handle, &mut |e| {
            let now = e.now();
            Ok((e.link.socket_closed(*socket, now), None))
        }),
        Request::LinkSend { handle, payload } => run(*handle, &mut |e| {
            let payload = decode(payload, zuse_remote::MAX_MESSAGE_BYTES)?;
            let now = e.now();
            let (accepted, effects) = e.link.send(&payload, now);
            Ok((effects, Some(accepted)))
        }),
        Request::LinkTick { handle } => run(*handle, &mut |e| {
            let now = e.now();
            Ok((e.link.tick(now), None))
        }),
        Request::LinkNetworkChanged { handle } => run(*handle, &mut |e| {
            let now = e.now();
            Ok((e.link.network_changed(&network_id(), now), None))
        }),
        Request::LinkClose { handle } => run(*handle, &mut |e| {
            let now = e.now();
            Ok((e.link.close(now), None))
        }),
        _ => return None,
    })
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
    fn a_link_connects_exchanges_messages_and_saves_routes() {
        use zuse_remote::session;
        let directory = tempfile::tempdir().unwrap();
        let invitation = zuse_remote::Invitation::generate().unwrap();
        let target = PairedTarget::new("https://host.example", invitation.clone()).unwrap();
        let open = call(json!({
            "op":"linkOpen",
            "link":target.export_link().unwrap(),
            "store":directory.path().to_str().unwrap(),
        }));
        assert!(!open.to_string().contains(&invitation.secret.expose()));
        let handle = open["handle"].as_u64().unwrap();
        let socket = open["open"][0]["socket"].as_u64().unwrap();
        assert_eq!(open["open"][0]["url"], target.socket_url());
        let records = |output: &Value| -> Vec<Vec<u8>> {
            output["write"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|write| write["records"].as_array().unwrap().clone())
                .map(|record| B64.decode(record.as_str().unwrap()).unwrap())
                .collect()
        };
        let opened = call(json!({"op":"linkSocketOpened","handle":handle,"socket":socket}));
        let (mut host, reply) =
            zuse_remote::accept(&invitation.id, &invitation.secret, &records(&opened)[0]).unwrap();
        let flight = call(
            json!({"op":"linkRecord","handle":handle,"socket":socket,"record":B64.encode(reply)}),
        );
        let flight = records(&flight);
        let challenge = host.open(&flight[0]).unwrap().unwrap();
        host.open(&flight[1]).unwrap().unwrap();
        let mut feed = |payload: &[u8]| {
            let mut last = json!({});
            for record in host.seal(payload).unwrap() {
                last = call(
                    json!({"op":"linkRecord","handle":handle,"socket":socket,"record":B64.encode(record)}),
                );
            }
            last
        };
        feed(
            &zuse_remote::heartbeat::response(&challenge)
                .unwrap()
                .unwrap(),
        );
        let learned = json!([{"kind":"lan","origin":"http://10.0.0.8:27657"}]);
        let attached = feed(&serde_json::to_vec(&json!({
            "id": session::ATTACH_ID,
            "ok": {"session":"s","resumed":false,"received":0,"caps":["lz4"],"engineInfo":{"deviceId":"host"},"routes":learned},
        })).unwrap());
        assert_eq!(attached["connected"], true);
        assert_eq!(attached["engineInfo"]["deviceId"], "host");
        assert_eq!(attached["status"]["state"], "connected");
        assert_eq!(attached["status"]["label"], "Direct");
        let saved = directory
            .path()
            .join(format!("{}.json", invitation.id.as_str()));
        assert!(std::fs::read_to_string(saved).unwrap().contains("10.0.0.8"));

        let text = br#"{"id":1,"item":"streamed text"}"#.repeat(100);
        let received = feed(&session::encode_message(&text, true));
        assert_eq!(
            B64.decode(received["deliver"][0].as_str().unwrap())
                .unwrap(),
            text
        );
        let sent = call(json!({"op":"linkSend","handle":handle,"payload":B64.encode(b"{}")}));
        assert_eq!(sent["accepted"], true);
        let closed = call(json!({"op":"linkClose","handle":handle}));
        assert_eq!(closed["ended"], "closed");
        assert!(
            execute(&json!({"op":"linkTick","handle":handle}).to_string())
                .get("error")
                .is_some()
        );
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
