//! Linux-only, exact pre-sleep socket identities. Never infer a destination from
//! a port alone: the caller also attests the selected idle native/config/MCP set.
//! Kernel cookies make a replacement connection an entirely different target.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};
use zeroize::Zeroizing;
const MAX_INPUT: usize = 64 * 1024;
const MAX_SOCKETS: usize = 32;
const MAX_FDS: usize = 1024;
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    version: u32,
    workspace_id: String,
    generation: i64,
    storage_incarnation_id: String,
    native_pid: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetRequest {
    version: u32,
    workspace_id: String,
    generation: i64,
    storage_incarnation_id: String,
    native_pid: u32,
    snapshot: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Process {
    pid: u32,
    uid: u32,
    start_ticks: u64,
    exe_device: u64,
    exe_inode: u64,
    network_namespace: u64,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Socket {
    fd: u32,
    inode: u64,
    family: u8,
    uid: u32,
    id: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    binding: Binding,
    process: Process,
    sockets: Vec<Socket>,
}
#[derive(Clone, Copy, Debug)]
struct Failure {
    code: &'static str,
    reset_count: usize,
}
type Result<T> = std::result::Result<T, Failure>;
fn failure(code: &'static str) -> Failure {
    Failure {
        code,
        reset_count: 0,
    }
}
fn require(value: bool, code: &'static str) -> Result<()> {
    if value { Ok(()) } else { Err(failure(code)) }
}
fn read(path: PathBuf, limit: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::new());
    File::open(path)
        .map_err(|_| failure("native_socket_process_changed"))?
        .take((limit + 1) as u64)
        .read_to_end(&mut out)
        .map_err(|_| failure("native_socket_process_changed"))?;
    require(out.len() <= limit, "native_socket_process_ambiguous")?;
    Ok(out)
}
fn binding(b: &Binding) -> Result<()> {
    require(
        b.version == 1 && b.generation > 0 && b.native_pid > 1,
        "native_socket_request_invalid",
    )?;
    for id in [&b.workspace_id, &b.storage_incarnation_id] {
        require(
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c)),
            "native_socket_request_invalid",
        )?;
    }
    Ok(())
}
fn process(pid: u32) -> Result<Process> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let status = read(root.join("status"), 32768)?;
    let uid_line = status
        .split(|c| *c == b'\n')
        .find(|l| l.starts_with(b"Uid:"))
        .ok_or_else(|| failure("native_socket_process_changed"))?;
    let uids = std::str::from_utf8(uid_line)
        .map_err(|_| failure("native_socket_process_changed"))?
        .split_whitespace()
        .skip(1)
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| failure("native_socket_process_changed"))?;
    require(
        uids.len() == 4 && uids[0] > 0 && uids.iter().all(|u| *u == uids[0]),
        "native_socket_owner_invalid",
    )?;
    let env = read(root.join("environ"), MAX_INPUT)?;
    for pair in env.split(|c| *c == 0) {
        if let Some(i) = pair.iter().position(|c| *c == b'=') {
            let key = &pair[..i];
            if [
                b"HTTP_PROXY".as_slice(),
                b"HTTPS_PROXY",
                b"ALL_PROXY",
                b"http_proxy",
                b"https_proxy",
                b"all_proxy",
                b"OPENAI_BASE_URL",
                b"CODEX_BASE_URL",
                b"OPENAI_API_BASE",
            ]
            .contains(&key)
                && pair.len() > i + 1
            {
                return Err(failure("native_socket_proxy_or_override_unsupported"));
            }
        }
    }
    let raw = read(root.join("stat"), 8192)?;
    let raw = std::str::from_utf8(&raw).map_err(|_| failure("native_socket_process_changed"))?;
    let end = raw
        .rfind(") ")
        .ok_or_else(|| failure("native_socket_process_changed"))?;
    let fields: Vec<_> = raw[end + 2..].split_whitespace().collect();
    require(
        fields.len() > 19 && !matches!(fields[0], "Z" | "X"),
        "native_socket_process_changed",
    )?;
    let start_ticks = fields[19]
        .parse()
        .map_err(|_| failure("native_socket_process_changed"))?;
    let exe = std::fs::read_link(root.join("exe"))
        .map_err(|_| failure("native_socket_process_changed"))?;
    require(
        exe.file_name() == Some(std::ffi::OsStr::new("codex")),
        "native_socket_executable_invalid",
    )?;
    let metadata = std::fs::metadata(root.join("exe"))
        .map_err(|_| failure("native_socket_process_changed"))?;
    require(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "native_socket_executable_invalid",
    )?;
    let network_namespace = std::fs::metadata(root.join("ns/net"))
        .map_err(|_| failure("native_socket_namespace_invalid"))?
        .ino();
    require(
        std::fs::metadata("/proc/self/ns/net")
            .map_err(|_| failure("native_socket_namespace_invalid"))?
            .ino()
            == network_namespace,
        "native_socket_namespace_invalid",
    )?;
    Ok(Process {
        pid,
        uid: uids[0],
        start_ticks,
        exe_device: metadata.dev(),
        exe_inode: metadata.ino(),
        network_namespace,
    })
}
fn caller(process: &Process) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        let origin = std::env::var("SUDO_UID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        require(origin == Some(process.uid), "native_socket_caller_invalid")
    } else {
        require(uid == process.uid, "native_socket_caller_invalid")
    }
}
fn fds(pid: u32) -> Result<BTreeMap<u64, Vec<u32>>> {
    let mut result = BTreeMap::<u64, Vec<u32>>::new();
    let entries = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map_err(|_| failure("native_socket_process_changed"))?;
    for (n, item) in entries.enumerate() {
        require(n < MAX_FDS, "native_socket_process_ambiguous")?;
        let item = item.map_err(|_| failure("native_socket_process_changed"))?;
        let Some(fd) = item
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(target) = std::fs::read_link(item.path()) else {
            continue;
        };
        let Some(inode) = target
            .to_str()
            .and_then(|v| v.strip_prefix("socket:["))
            .and_then(|v| v.strip_suffix(']'))
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        result.entry(inode).or_default().push(fd);
    }
    Ok(result)
}
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || a == 100 && (64..=127).contains(&b)
                || a == 169 && b == 254
                || a == 172 && (16..=31).contains(&b)
                || a == 192 && (b == 0 || b == 168 || b == 88 && c == 99)
                || a == 198 && (b == 18 || b == 19 || b == 51 && c == 100)
                || a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let x = ip.octets();
            x[0] & 0xe0 == 0x20
                && !(x[0] == 0x20 && x[1] == 0x01 && x[2] <= 1)
                && !(x[..4] == [0x20, 0x01, 0x0d, 0xb8])
                && !(x[..2] == [0x20, 0x02])
                && !(x[..2] == [0x3f, 0xff])
        }
    }
}
fn remote(socket: &Socket) -> Option<IpAddr> {
    if socket.id.len() != 48 {
        return None;
    }
    match socket.family as i32 {
        libc::AF_INET => Some(IpAddr::V4(Ipv4Addr::new(
            socket.id[20],
            socket.id[21],
            socket.id[22],
            socket.id[23],
        ))),
        libc::AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(&socket.id[20..36]).ok()?,
        ))),
        _ => None,
    }
}
fn eligible_peer(socket: &Socket) -> bool {
    socket.inode > 0
        && socket.id.len() == 48
        && u16::from_be_bytes([socket.id[2], socket.id[3]]) == 443
        && remote(socket).is_some_and(public)
}
fn valid_socket(socket: &Socket) -> bool {
    eligible_peer(socket) && socket.id[40..48] != [255; 8] && socket.id[40..48] != [0; 8]
}
#[derive(Clone)]
struct Diag {
    socket: Socket,
    state: u8,
}
struct Netlink {
    fd: OwnedFd,
    seq: u32,
}
fn align(n: usize) -> usize {
    (n + 3) & !3
}
fn u32at(b: &[u8], n: usize) -> u32 {
    u32::from_ne_bytes(b[n..n + 4].try_into().unwrap())
}
impl Netlink {
    fn open() -> Result<Self> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_SOCK_DIAG,
            )
        };
        if fd < 0 {
            return Err(failure("native_socket_diag_unsupported"));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let time = libc::timeval {
            tv_sec: 1,
            tv_usec: 0,
        };
        require(
            unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &time as *const _ as *const _,
                    std::mem::size_of_val(&time) as u32,
                )
            } == 0,
            "native_socket_diag_unsupported",
        )?;
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as u16;
        require(
            unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    &address as *const _ as *const _,
                    std::mem::size_of_val(&address) as u32,
                )
            } == 0,
            "native_socket_diag_unsupported",
        )?;
        Ok(Self { fd, seq: 0 })
    }
    fn request(
        &mut self,
        kind: u16,
        family: u8,
        id: Option<&[u8]>,
        dump: bool,
    ) -> Result<Vec<Diag>> {
        self.seq = self.seq.wrapping_add(1);
        let mut bytes = vec![0u8; 72];
        bytes[..4].copy_from_slice(&72u32.to_ne_bytes());
        bytes[4..6].copy_from_slice(&kind.to_ne_bytes());
        bytes[6..8].copy_from_slice(&(if dump { 0x301u16 } else { 5u16 }).to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.seq.to_ne_bytes());
        bytes[16] = family;
        bytes[17] = libc::IPPROTO_TCP as u8;
        bytes[20..24].copy_from_slice(&u32::MAX.to_ne_bytes());
        if let Some(id) = id {
            require(id.len() == 48, "native_socket_snapshot_invalid")?;
            bytes[24..72].copy_from_slice(id);
        } else {
            bytes[64..72].fill(255);
        }
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as u16;
        require(
            unsafe {
                libc::sendto(
                    self.fd.as_raw_fd(),
                    bytes.as_ptr() as *const _,
                    bytes.len(),
                    0,
                    &address as *const _ as *const _,
                    std::mem::size_of_val(&address) as u32,
                )
            } == bytes.len() as isize,
            "native_socket_diag_unsupported",
        )?;
        let mut rows = Vec::new();
        for _ in 0..128 {
            let mut data = [0u8; 65536];
            let mut peer: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            let mut peer_len = std::mem::size_of_val(&peer) as libc::socklen_t;
            let n = unsafe {
                libc::recvfrom(
                    self.fd.as_raw_fd(),
                    data.as_mut_ptr() as *mut _,
                    data.len(),
                    libc::MSG_TRUNC,
                    &mut peer as *mut _ as *mut _,
                    &mut peer_len,
                )
            };
            if n < 0 {
                return Err(failure(if dump {
                    "native_socket_diag_timeout"
                } else {
                    "native_socket_destroy_uncertain"
                }));
            }
            require(
                peer_len as usize == std::mem::size_of_val(&peer)
                    && peer.nl_family == libc::AF_NETLINK as u16
                    && peer.nl_pid == 0
                    && peer.nl_groups == 0,
                "native_socket_diag_ambiguous",
            )?;
            require(n as usize <= data.len(), "native_socket_diag_ambiguous")?;
            let mut offset = 0;
            while offset + 16 <= n as usize {
                let size = u32at(&data, offset) as usize;
                require(
                    size >= 16 && offset + size <= n as usize,
                    "native_socket_diag_ambiguous",
                )?;
                let h = &data[offset..offset + size];
                let kind = u16::from_ne_bytes(h[4..6].try_into().unwrap());
                let flags = u16::from_ne_bytes(h[6..8].try_into().unwrap());
                require(u32at(h, 8) == self.seq, "native_socket_diag_ambiguous")?;
                require(flags & 0x10 == 0, "native_socket_diag_ambiguous")?;
                if kind == 3 {
                    return Ok(rows);
                }
                if kind == 2 {
                    require(size >= 20, "native_socket_diag_ambiguous")?;
                    let code = i32::from_ne_bytes(h[16..20].try_into().unwrap());
                    if code == 0 {
                        if !dump {
                            return Ok(rows);
                        }
                    } else {
                        return Err(failure(match -code {
                            libc::EPERM | libc::EACCES => "native_socket_permission_required",
                            libc::ENOENT => "native_socket_already_gone",
                            libc::ESTALE => "native_socket_identity_changed",
                            _ => "native_socket_diag_unsupported",
                        }));
                    }
                } else if kind == 20 {
                    require(size >= 88, "native_socket_diag_ambiguous")?;
                    let payload = &h[16..];
                    require(payload[0] == family, "native_socket_diag_ambiguous")?;
                    rows.push(Diag {
                        state: payload[1],
                        socket: Socket {
                            fd: 0,
                            inode: u32at(payload, 68) as u64,
                            family,
                            uid: u32at(payload, 64),
                            id: payload[4..52].to_vec(),
                        },
                    });
                    require(rows.len() <= 8192, "native_socket_diag_ambiguous")?;
                }
                offset += align(size);
            }
        }
        Err(failure("native_socket_diag_ambiguous"))
    }
    fn all(&mut self) -> Result<Vec<Diag>> {
        let mut rows = self.request(20, libc::AF_INET as u8, None, true)?;
        rows.extend(self.request(20, libc::AF_INET6 as u8, None, true)?);
        Ok(rows)
    }
    fn destroy(&mut self, socket: &Socket) -> Result<()> {
        self.request(21, socket.family, Some(&socket.id), false)
            .map(|_| ())
    }
}
fn capture_socket(row: Diag, owned: &BTreeMap<u64, Vec<u32>>, uid: u32) -> Result<Option<Socket>> {
    let Some(fds) = owned.get(&row.socket.inode) else {
        return Ok(None);
    };
    if row.state != 1 || !eligible_peer(&row.socket) {
        return Ok(None);
    }
    // No-cookie kernels are unsupported, rather than an apparently empty
    // snapshot which would silently retain the stale public connection.
    require(row.socket.uid == uid, "native_socket_owner_invalid")?;
    require(valid_socket(&row.socket), "native_socket_diag_unsupported")?;
    require(fds.len() == 1, "native_socket_process_ambiguous")?;
    let mut socket = row.socket;
    socket.fd = fds[0];
    Ok(Some(socket))
}
fn capture(b: Binding) -> Result<serde_json::Value> {
    binding(&b)?;
    let identity = process(b.native_pid)?;
    caller(&identity)?;
    let owned = fds(b.native_pid)?;
    let mut diag = Netlink::open()?;
    let rows = diag.all()?;
    let mut sockets = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        if let Some(socket) = capture_socket(row, &owned, identity.uid)? {
            require(seen.insert(socket.inode), "native_socket_process_ambiguous")?;
            sockets.push(socket);
            require(
                sockets.len() <= MAX_SOCKETS,
                "native_socket_process_ambiguous",
            )?;
        }
    }
    let current_fds = fds(b.native_pid)?;
    for socket in &sockets {
        require(
            current_fds
                .get(&socket.inode)
                .is_some_and(|fds| fds.len() == 1 && fds[0] == socket.fd),
            "native_socket_identity_changed",
        )?;
    }
    require(
        process(b.native_pid)? == identity,
        "native_socket_process_changed",
    )?;
    let n = sockets.len();
    let snapshot = Snapshot {
        binding: b,
        process: identity,
        sockets,
    };
    let bytes =
        serde_json::to_vec(&snapshot).map_err(|_| failure("native_socket_snapshot_invalid"))?;
    require(
        bytes.len() < MAX_INPUT / 2,
        "native_socket_snapshot_invalid",
    )?;
    let encoded = B64.encode(bytes);
    require(
        encoded.len() <= MAX_INPUT / 2,
        "native_socket_snapshot_invalid",
    )?;
    Ok(serde_json::json!({"version":1,"status":"captured","socketCount":n,"snapshot":encoded}))
}
fn reset(req: ResetRequest) -> Result<serde_json::Value> {
    let expected = Binding {
        version: req.version,
        workspace_id: req.workspace_id,
        generation: req.generation,
        storage_incarnation_id: req.storage_incarnation_id,
        native_pid: req.native_pid,
    };
    binding(&expected)?;
    require(
        req.snapshot.len() <= MAX_INPUT / 2,
        "native_socket_snapshot_invalid",
    )?;
    let bytes = B64
        .decode(req.snapshot)
        .map_err(|_| failure("native_socket_snapshot_invalid"))?;
    let snapshot: Snapshot =
        serde_json::from_slice(&bytes).map_err(|_| failure("native_socket_snapshot_invalid"))?;
    require(
        snapshot.binding == expected
            && snapshot.process.pid == expected.native_pid
            && snapshot.sockets.len() <= MAX_SOCKETS,
        "native_socket_snapshot_invalid",
    )?;
    let current = process(expected.native_pid)?;
    caller(&current)?;
    require(current == snapshot.process, "native_socket_process_changed")?;
    let owned = fds(current.pid)?;
    let mut netlink = Netlink::open()?;
    let rows = netlink.all()?;
    let mut planned = Vec::new();
    let mut gone = 0;
    let mut seen = BTreeSet::new();
    for socket in &snapshot.sockets {
        require(
            valid_socket(socket) && socket.uid == current.uid && seen.insert(socket.inode),
            "native_socket_snapshot_invalid",
        )?;
        let matches: Vec<_> = rows
            .iter()
            .filter(|r| r.socket.inode == socket.inode)
            .collect();
        if matches.is_empty() {
            gone += 1;
            continue;
        }
        require(matches.len() == 1, "native_socket_process_ambiguous")?;
        let row = matches[0];
        require(
            row.socket.family == socket.family
                && row.socket.uid == socket.uid
                && row.socket.id == socket.id
                && row.state != 10,
            "native_socket_identity_changed",
        )?;
        require(
            owned
                .get(&socket.inode)
                .is_some_and(|fds| fds.len() == 1 && fds[0] == socket.fd),
            "native_socket_identity_changed",
        )?;
        planned.push(socket);
    }
    require(
        process(current.pid)? == snapshot.process,
        "native_socket_process_changed",
    )?;
    let mut destroyed = 0;
    for socket in planned {
        let result = (|| {
            require(
                process(current.pid)? == snapshot.process,
                "native_socket_process_changed",
            )?;
            let owned = fds(current.pid)?;
            require(
                owned
                    .get(&socket.inode)
                    .is_some_and(|fds| fds.len() == 1 && fds[0] == socket.fd),
                "native_socket_identity_changed",
            )?;
            netlink.destroy(socket)
        })();
        match result {
            Ok(()) => destroyed += 1,
            Err(e) if e.code == "native_socket_already_gone" => gone += 1,
            Err(mut e) => {
                e.reset_count = destroyed;
                return Err(e);
            }
        }
    }
    Ok(
        serde_json::json!({"version":1,"status":"reset","socketCount":snapshot.sockets.len(),"resetCount":destroyed,"alreadyGoneCount":gone}),
    )
}
/// Early synchronous CLI: does not initialize the engine, read auth files, or
/// send any model request. Error output contains only stable codes and counts.
pub fn run_cli(mode: &str) -> anyhow::Result<()> {
    let result = (|| {
        let mut bytes = Zeroizing::new(Vec::new());
        std::io::stdin()
            .lock()
            .take((MAX_INPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| failure("native_socket_request_invalid"))?;
        require(bytes.len() <= MAX_INPUT, "native_socket_request_invalid")?;
        match mode {
            "capture" => capture(
                serde_json::from_slice(&bytes)
                    .map_err(|_| failure("native_socket_request_invalid"))?,
            ),
            "reset" => reset(
                serde_json::from_slice(&bytes)
                    .map_err(|_| failure("native_socket_request_invalid"))?,
            ),
            _ => Err(failure("native_socket_request_invalid")),
        }
    })();
    let (value, code) = match result {
        Ok(v) => (v, None),
        Err(e) => (
            serde_json::json!({"version":1,"status":"recovery-required","code":e.code,"resetCount":e.reset_count}),
            Some(e.code),
        ),
    };
    serde_json::to_writer(std::io::stdout().lock(), &value)?;
    std::io::stdout().lock().write_all(b"\n")?;
    if let Some(code) = code {
        anyhow::bail!("{code}");
    }
    Ok(())
}
#[cfg(test)]
#[path = "native_sockets/tests.rs"]
mod tests;
