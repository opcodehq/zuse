use super::*;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
#[test]
fn remote_selection_excludes_internal_documentation_and_listeners() {
    for addr in [
        "127.0.0.1",
        "10.0.0.1",
        "100.64.0.1",
        "169.254.1.1",
        "172.16.1.1",
        "192.168.1.1",
        "192.0.2.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "2001:db8::1",
        "::ffff:10.0.0.1",
    ] {
        assert!(!public(addr.parse().unwrap()), "{addr}");
    }
    for addr in ["8.8.8.8", "104.18.0.1", "2606:4700::1", "::ffff:8.8.8.8"] {
        assert!(public(addr.parse().unwrap()), "{addr}");
    }
    let mut s = Socket {
        fd: 5,
        inode: 10,
        family: libc::AF_INET as u8,
        uid: 10001,
        id: vec![0; 48],
    };
    s.id[2..4].copy_from_slice(&443u16.to_be_bytes());
    s.id[20..24].copy_from_slice(&[8, 8, 8, 8]);
    s.id[40..48].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(valid_socket(&s));
    s.id[2..4].copy_from_slice(&0u16.to_be_bytes());
    assert!(!valid_socket(&s));
}
#[test]
fn malformed_requests_and_snapshots_fail_before_kernel_mutation() {
    assert_eq!(
        binding(&Binding {
            version: 1,
            workspace_id: "fixture".into(),
            generation: 0,
            storage_incarnation_id: "disk".into(),
            native_pid: 2
        })
        .unwrap_err()
        .code,
        "native_socket_request_invalid"
    );
    let req = ResetRequest {
        version: 1,
        workspace_id: "fixture".into(),
        generation: 1,
        storage_incarnation_id: "disk".into(),
        native_pid: 2,
        snapshot: "not a snapshot".into(),
    };
    assert_eq!(
        reset(req).unwrap_err().code,
        "native_socket_snapshot_invalid"
    );
}
#[test]
fn kernel_dump_is_read_only_and_bounded() {
    let mut diag = Netlink::open().unwrap();
    let rows = diag.all().unwrap();
    assert!(rows.len() <= 16384);
}
struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(binary: &std::path::Path, target: &str, proxy: bool) -> Guard {
    let mut c = Command::new(binary);
    c.args([
        "--exact",
        "native_sockets::tests::kernel_client",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ])
    .env_clear()
    .env("ZUSE_NATIVE_SOCKET_KERNEL_TEST", "1")
    .env("ZUSE_NATIVE_SOCKET_TARGET", target)
    .uid(10001)
    .gid(10001)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    if proxy {
        c.env("HTTPS_PROXY", "http://127.0.0.1:9");
    }
    Guard(c.spawn().unwrap())
}
fn accept(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((mut s, _)) => {
                s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut ready = [0; 5];
                s.read_exact(&mut ready).unwrap();
                assert_eq!(&ready, b"ready");
                return s;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "fixture connection missing");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("fixture listener: {e}"),
        }
    }
}
fn ping(stream: &mut TcpStream) {
    stream.write_all(b"p").unwrap();
    let mut byte = [0];
    stream.read_exact(&mut byte).unwrap();
    assert_eq!(byte[0], b'p');
}
fn context(pid: u32) -> Binding {
    Binding {
        version: 1,
        workspace_id: "workspace-fixture".into(),
        generation: 3,
        storage_incarnation_id: "disk-fixture".into(),
        native_pid: pid,
    }
}
fn request(b: &Binding, snapshot: String) -> ResetRequest {
    ResetRequest {
        version: b.version,
        workspace_id: b.workspace_id.clone(),
        generation: b.generation,
        storage_incarnation_id: b.storage_incarnation_id.clone(),
        native_pid: b.native_pid,
        snapshot,
    }
}
fn encode(s: &Snapshot) -> String {
    B64.encode(serde_json::to_vec(s).unwrap())
}
#[test]
#[ignore = "run only as root in the controlled isolated network namespace wrapper"]
fn kernel_exact_cookie_and_process_fences_precede_real_destroy() {
    // Only inside the root network-namespace wrapper; `--include-ignored`
    // elsewhere (CI) has nothing to exercise.
    if std::env::var("ZUSE_NATIVE_SOCKET_KERNEL_TEST").as_deref() != Ok("1") {
        return;
    }
    assert_eq!(unsafe { libc::geteuid() }, 0);
    assert_ne!(
        std::fs::metadata("/proc/self/ns/net").unwrap().ino(),
        std::fs::metadata("/proc/1/ns/net").unwrap().ino(),
        "never put fixture addresses on host network"
    );
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let binary = temp.path().join("codex");
    std::fs::copy(std::env::current_exe().unwrap(), &binary).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o555)).unwrap();
    let listener = TcpListener::bind("8.8.8.8:443").unwrap();
    let client = child(&binary, "8.8.8.8:443", false);
    let mut stream = accept(&listener);
    let binding = context(client.0.id());
    let captured = if std::env::var_os("ZUSE_NATIVE_SOCKET_GUEST_BINARY").is_some() {
        guest_cli("capture", &serde_json::to_value(&binding).unwrap())
    } else {
        capture(binding.clone()).unwrap()
    };
    assert_eq!(captured["socketCount"], 1);
    let opaque = captured["snapshot"].as_str().unwrap().to_owned();
    let snapshot: Snapshot = serde_json::from_slice(&B64.decode(&opaque).unwrap()).unwrap();
    let mut wrong: Snapshot = serde_json::from_slice(&B64.decode(&opaque).unwrap()).unwrap();
    wrong.process.start_ticks += 1;
    assert_eq!(
        reset(request(&binding, encode(&wrong))).unwrap_err().code,
        "native_socket_process_changed"
    );
    ping(&mut stream);
    wrong = serde_json::from_slice(&B64.decode(&opaque).unwrap()).unwrap();
    wrong.sockets[0].id[40] ^= 1;
    assert_eq!(
        reset(request(&binding, encode(&wrong))).unwrap_err().code,
        "native_socket_identity_changed"
    );
    ping(&mut stream);
    wrong = serde_json::from_slice(&B64.decode(&opaque).unwrap()).unwrap();
    wrong.sockets[0].fd += 100;
    assert_eq!(
        reset(request(&binding, encode(&wrong))).unwrap_err().code,
        "native_socket_identity_changed"
    );
    ping(&mut stream);
    wrong = serde_json::from_slice(&B64.decode(&opaque).unwrap()).unwrap();
    wrong.binding.generation += 1;
    assert_eq!(
        reset(request(&binding, encode(&wrong))).unwrap_err().code,
        "native_socket_snapshot_invalid"
    );
    ping(&mut stream);
    // An unprivileged same-user caller can inspect but cannot destroy the socket.
    let mut unpriv = Command::new(&binary)
        .args([
            "--exact",
            "native_sockets::tests::kernel_unprivileged_reset",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("ZUSE_NATIVE_SOCKET_KERNEL_TEST", "1")
        .uid(10001)
        .gid(10001)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes = serde_json::to_vec(&request_json(&binding, &opaque)).unwrap();
    unpriv.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = unpriv.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "unprivileged denial control failed"
    );
    ping(&mut stream);
    // Proxy environment changes route semantics; never apply a public-port guess.
    let mut proxy = child(&binary, "8.8.8.8:443", true);
    let mut proxy_stream = accept(&listener);
    assert_eq!(
        capture(context(proxy.0.id())).unwrap_err().code,
        "native_socket_proxy_or_override_unsupported"
    );
    ping(&mut proxy_stream);
    proxy_stream.write_all(b"q").unwrap();
    let _ = proxy.0.wait();
    // A private loopback transport is excluded and is never interrupted.
    let local = TcpListener::bind("127.0.0.1:443").unwrap();
    let mut local_child = child(&binary, "127.0.0.1:443", false);
    let mut local_stream = accept(&local);
    let local_b = context(local_child.0.id());
    let local_capture = capture(local_b.clone()).unwrap();
    assert_eq!(local_capture["socketCount"], 0);
    assert_eq!(
        reset(request(
            &local_b,
            local_capture["snapshot"].as_str().unwrap().into()
        ))
        .unwrap()["resetCount"],
        0
    );
    ping(&mut local_stream);
    local_stream.write_all(b"q").unwrap();
    let _ = local_child.0.wait();
    assert_eq!(snapshot.sockets.len(), 1);
    let result = if std::env::var_os("ZUSE_NATIVE_SOCKET_GUEST_BINARY").is_some() {
        guest_cli("reset", &request_json(&binding, &opaque))
    } else {
        reset(request(&binding, opaque)).unwrap()
    };
    assert_eq!(result["resetCount"], 1);
    assert_eq!(result["alreadyGoneCount"], 0);
    let end = Instant::now() + Duration::from_secs(3);
    let mut client = client;
    loop {
        if let Some(status) = client.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "client must observe exactly the controlled close"
            );
            break;
        }
        assert!(
            Instant::now() < end,
            "kernel did not retire exact TCP connection"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn request_json(b: &Binding, snapshot: &str) -> serde_json::Value {
    serde_json::json!({"version":b.version,"workspaceId":b.workspace_id,"generation":b.generation,"storageIncarnationId":b.storage_incarnation_id,"nativePid":b.native_pid,"snapshot":snapshot})
}
#[test]
#[ignore = "controlled child only"]
fn kernel_unprivileged_reset() {
    // Only inside the root network-namespace wrapper; `--include-ignored`
    // elsewhere (CI) has nothing to exercise.
    if std::env::var("ZUSE_NATIVE_SOCKET_KERNEL_TEST").as_deref() != Ok("1") {
        return;
    }
    assert_eq!(unsafe { libc::geteuid() }, 10001);
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let result = reset(serde_json::from_slice(&input).unwrap()).unwrap_err();
    assert_eq!(result.code, "native_socket_permission_required");
    assert_eq!(result.reset_count, 0);
}
#[test]
#[ignore = "controlled child only"]
fn kernel_client() {
    // Only inside the root network-namespace wrapper; `--include-ignored`
    // elsewhere (CI) has nothing to exercise.
    if std::env::var("ZUSE_NATIVE_SOCKET_KERNEL_TEST").as_deref() != Ok("1") {
        return;
    }
    assert_eq!(unsafe { libc::geteuid() }, 10001);
    let address = std::env::var("ZUSE_NATIVE_SOCKET_TARGET").unwrap();
    let mut socket =
        TcpStream::connect_timeout(&address.parse().unwrap(), Duration::from_secs(2)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket.write_all(b"ready").unwrap();
    loop {
        let mut input = [0];
        match socket.read(&mut input) {
            Ok(0) => return,
            Ok(_) => match input[0] {
                b'p' => socket.write_all(b"p").unwrap(),
                b'q' => return,
                _ => panic!("invalid controlled packet"),
            },
            Err(e) => {
                assert!(
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                    ),
                    "unexpected kernel fixture failure"
                );
                return;
            }
        }
    }
}

#[test]
fn kernel_no_cookie_or_wrong_owner_cannot_be_silently_omitted() {
    let mut socket = Socket {
        fd: 0,
        inode: 42,
        family: libc::AF_INET as u8,
        uid: 10001,
        id: vec![0; 48],
    };
    socket.id[2..4].copy_from_slice(&443u16.to_be_bytes());
    socket.id[20..24].copy_from_slice(&[8, 8, 8, 8]);
    socket.id[40..48].fill(255);
    let owned = BTreeMap::from([(42, vec![7])]);
    assert_eq!(
        capture_socket(
            Diag {
                socket: socket.clone(),
                state: 1
            },
            &owned,
            10001
        )
        .err()
        .unwrap()
        .code,
        "native_socket_diag_unsupported"
    );
    socket.id[40..48].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        capture_socket(
            Diag {
                socket: socket.clone(),
                state: 1
            },
            &owned,
            10002
        )
        .err()
        .unwrap()
        .code,
        "native_socket_owner_invalid"
    );
    assert!(
        capture_socket(
            Diag {
                socket: socket.clone(),
                state: 10
            },
            &owned,
            10001
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        capture_socket(
            Diag { socket, state: 1 },
            &BTreeMap::from([(42, vec![7, 8])]),
            10001
        )
        .err()
        .unwrap()
        .code,
        "native_socket_process_ambiguous"
    );
}

fn guest_cli(mode: &str, body: &serde_json::Value) -> serde_json::Value {
    let binary = std::env::var_os("ZUSE_NATIVE_SOCKET_GUEST_BINARY").unwrap();
    let mut child = Command::new(binary)
        .args(["native-sockets", mode])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SUDO_UID", "10001")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(body).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "captured guest CLI failed safe fixture mode"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(
        value["status"],
        if mode == "capture" {
            "captured"
        } else {
            "reset"
        }
    );
    value
}
