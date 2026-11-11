#![cfg(unix)]
//! Real server/agent boundary: independent ephemeral listeners and graceful exit.
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};
use zuse_rpc::{RpcClient, connect_ws, methods};

const ZUSE: &str = env!("CARGO_BIN_EXE_zuse-cli");

/// The same binary under its old name, as packages still install it.
fn legacy_server(directory: &Path) -> PathBuf {
    let link = directory.join("zuse-server");
    if !link.exists() {
        std::os::unix::fs::symlink(ZUSE, &link).unwrap();
    }
    link
}

struct Server {
    child: Child,
    port: u16,
    _output: BufReader<ChildStdout>,
}
impl Server {
    async fn start(profile: &Path, bin: &Path, capture: &Path) -> Self {
        let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        let mut child = Command::new(ZUSE)
            .args([
                "serve",
                "--listen",
                "127.0.0.1:0",
                "--ipc-port",
                "0",
                "--data-dir",
            ])
            .current_dir(profile.parent().unwrap())
            .arg(profile.file_name().unwrap())
            .env("PATH", path)
            .env("CLAUDE_CODE_EXECUTABLE", bin.join("claude"))
            .env("ZUSE_STARTUP_CAPTURE", capture)
            .env("ZUSE_STARTUP_TOOL_FRAME", bin.join("tool-result.json"))
            .env_remove("ZUSE_BROWSER_TOKEN")
            .env_remove("ZUSE_PUBLIC_ORIGIN")
            .env_remove("ZUSE_TLS_CERT")
            .env_remove("ZUSE_TLS_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let port = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(
                    output.read_line(&mut line).await.unwrap() > 0,
                    "server ended before readiness"
                );
                if let Some(port) = line.trim().strip_prefix("Agent tools listen on 127.0.0.1:") {
                    break port.parse::<u16>().unwrap();
                }
            }
        })
        .await
        .unwrap();
        Self {
            child,
            port,
            _output: output,
        }
    }
    async fn client(&self) -> RpcClient {
        connect_ws(&format!("ws://127.0.0.1:{}", self.port))
            .await
            .unwrap()
    }
    async fn stop(&mut self) {
        let pid = self.child.id().unwrap();
        assert!(
            Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "SIGTERM must drain the server and exit cleanly: {status}"
        );
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_servers_inject_their_own_ephemeral_rpc_endpoint_and_drain_on_sigterm() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let full_output = "🙂 persisted tool output\n".repeat(6000);
    std::fs::write(bin.join("tool-result.json"),serde_json::to_vec(&json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"fixture-tool","content":full_output}]}})).unwrap()).unwrap();
    let executable = bin.join("claude");
    std::fs::write(&executable,r##"#!/bin/sh
case "$*" in *--version*) echo '2.1.0'; exit 0;; esac
previous=''
for argument in "$@"; do
  if [ "$previous" = '--mcp-config' ]; then
    case "$argument" in *'"zuse"'*) printf '%s' "$argument" > "$ZUSE_STARTUP_CAPTURE";; esac
  fi
  previous="$argument"
done
IFS= read -r prompt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"startup-fixture","model":"fixture","tools":[]}'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"fixture-tool","name":"Bash","input":{"command":"printf fixture"}}]}}'
cat "$ZUSE_STARTUP_TOOL_FRAME"
printf '\n'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"startup fixture completed"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","session_id":"startup-fixture","is_error":false,"result":"startup fixture completed"}'
"##).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let capture = root.path().join("mcp.json");
    let mut first = Server::start(&root.path().join("one"), &bin, &capture).await;
    let mut second = Server::start(
        &root.path().join("two"),
        &bin,
        &root.path().join("other.json"),
    )
    .await;
    assert_ne!(first.port, 0);
    assert_ne!(first.port, second.port);
    let client = first.client().await;
    let other = second.client().await;
    let info = client.call(methods::ENGINE_INFO, json!({})).await.unwrap();
    let other_info = other.call(methods::ENGINE_INFO, json!({})).await.unwrap();
    assert_ne!(info["deviceId"], other_info["deviceId"]);
    client.call(methods::MUTATE,json!({"op":"createChat","chatId":"startup-chat","deviceId":info["deviceId"],"config":{"harness":"claude-code","model":null,"reasoning":null,"sandbox":"workspace-write"}})).await.unwrap();
    let files = client
        .call(
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":"startup-chat","directory":""}),
        )
        .await;
    assert!(
        files.is_ok(),
        "relative profiles must resolve scratch workspaces: {files:?}"
    );
    let mut messages = client
        .subscribe_scoped(
            methods::WATCH_DOC_MESSAGES,
            json!({"chatId":"startup-chat"}),
        )
        .await
        .unwrap();
    client.call(methods::QUEUE_COMMAND,json!({"chatId":"startup-chat","command":{"kind":"run","messageId":"first","request":{"prompt":"Startup boundary fixture","harness":"claude-code","model":null,"reasoning":null,"cwd":root.path(),"sandbox":"workspace-write","autoApprove":true,"resume":null,"attachments":[]}}})).await.unwrap();
    let config = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(bytes) = std::fs::read(&capture) {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    break value;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let injected = &config["mcpServers"]["zuse"];
    assert_eq!(injected["env"]["ZUSE_IPC_PORT"], first.port.to_string());
    assert_eq!(injected["env"]["ZUSE_CHAT_ID"], "startup-chat");
    assert_eq!(injected["env"]["ZUSE_DEVICE_ID"], info["deviceId"]);
    assert_eq!(injected["args"], json!(["mcp"]));
    let routed = connect_ws(&format!(
        "ws://127.0.0.1:{}",
        injected["env"]["ZUSE_IPC_PORT"].as_str().unwrap()
    ))
    .await
    .unwrap();
    assert_eq!(
        routed.call(methods::ENGINE_INFO, json!({})).await.unwrap()["deviceId"],
        info["deviceId"]
    );
    fn find_ref(value: &Value) -> Option<String> {
        if value["kind"] == "tool" {
            if let Some(reference) = value["outputRef"].as_str() {
                return Some(reference.into());
            }
        }
        match value {
            Value::Object(map) => map.values().find_map(find_ref),
            Value::Array(items) => items.iter().find_map(find_ref),
            _ => None,
        }
    }
    let reference = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = messages
                .recv()
                .await
                .expect("transcript ended before tool output");
            if let Some(reference) = find_ref(&frame) {
                break reference;
            }
        }
    })
    .await
    .unwrap();
    drop(messages);
    let first_page = client
        .call(
            methods::FETCH_TOOL_BLOB,
            json!({"blobRef":reference,"offset":0,"limit":65536}),
        )
        .await
        .unwrap();
    assert_eq!(first_page["done"], false);
    assert!(first_page["text"].as_str().unwrap().len() <= 65536);
    assert!(
        other
            .call(methods::FETCH_TOOL_BLOB, json!({"blobRef":reference}))
            .await
            .is_err(),
        "tool results must stay profile-local"
    );
    let token = std::fs::read(root.path().join("one/browser-access-token")).unwrap();
    first.stop().await;
    second.stop().await;
    let mut reopened = Server::start(&root.path().join("one"), &bin, &capture).await;
    assert_eq!(
        std::fs::read(root.path().join("one/browser-access-token")).unwrap(),
        token
    );
    let client = reopened.client().await;
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let page = client
            .call(
                methods::FETCH_TOOL_BLOB,
                json!({"blobRef":reference,"offset":offset,"limit":65536}),
            )
            .await
            .unwrap();
        text.push_str(page["text"].as_str().unwrap());
        offset = page["nextOffset"].as_u64().unwrap();
        if page["done"] == true {
            break;
        }
    }
    assert_eq!(text, full_output);
    assert!(
        client
            .call(
                methods::FETCH_TOOL_BLOB,
                json!({"blobRef":reference,"offset":1})
            )
            .await
            .is_err()
    );
    reopened.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scratch_checkout_review_and_editing_reject_stale_disk_state() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("review-profile");
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let mut server = Server::start(&profile, &bin, &root.path().join("capture")).await;
    let cwd = profile.join("scratch").join("review-fixture");
    std::fs::create_dir_all(&cwd).unwrap();
    async fn git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    git(&cwd, &["init", "-q"]).await;
    std::fs::write(cwd.join("review.txt"), "original line\n").unwrap();
    git(&cwd, &["add", "review.txt"]).await;
    git(
        &cwd,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    )
    .await;
    std::fs::write(cwd.join("review.txt"), "changed line\n").unwrap();
    let client = server.client().await;
    let info = client.call(methods::ENGINE_INFO, json!({})).await.unwrap();
    let portable = "~/.zuse/scratch/review-fixture";
    client.call(methods::MUTATE,json!({"op":"createChat","chatId":"review-chat","deviceId":info["deviceId"],"cwd":portable,"config":{"harness":"claude-code","model":null,"reasoning":null,"sandbox":"workspace-write"}})).await.unwrap();
    let diff = client
        .call(
            methods::GET_CHECKOUT_DIFF,
            json!({"cwd":portable,"chatId":"review-chat","mode":"workingTree"}),
        )
        .await
        .unwrap();
    assert_eq!(diff["files"][0]["path"], "review.txt");
    let request = json!({"cwd":portable,"checkoutId":diff["checkoutId"],"chatId":"review-chat","path":"review.txt","mode":"workingTree","diffChecksum":diff["checksum"]});
    let file_diff = client
        .call(methods::GET_CHECKOUT_FILE_DIFF_TEXT, request.clone())
        .await
        .unwrap();
    assert_eq!(file_diff["stale"], false);
    assert_eq!(file_diff["oldText"], "original line\n");
    assert_eq!(file_diff["newText"], "changed line\n");
    let read = client
        .call(
            methods::READ_WORKSPACE_FILE,
            json!({"chatId":"review-chat","path":"review.txt"}),
        )
        .await
        .unwrap();
    std::fs::write(cwd.join("review.txt"), "external edit\n").unwrap();
    let stale = client
        .call(methods::GET_CHECKOUT_FILE_DIFF_TEXT, request)
        .await
        .unwrap();
    assert_eq!(stale["stale"], true);
    let conflict=client.call(methods::WRITE_WORKSPACE_FILE,json!({"chatId":"review-chat","path":"review.txt","expectedCheckoutId":read["checkoutId"],"expectedContentHash":read["contentHash"],"text":"unsaved editor text\n","encoding":"utf8","lineEnding":"lf"})).await.unwrap();
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(
        std::fs::read_to_string(cwd.join("review.txt")).unwrap(),
        "external edit\n"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_https_authenticates_browser_rpc_and_drains_open_websockets_on_sigterm() {
    use futures::{SinkExt, StreamExt};
    use std::sync::Arc;
    use tokio_rustls::rustls;
    use tokio_tungstenite::{
        Connector,
        tungstenite::{Message, client::IntoClientRequest},
    };
    let root = tempfile::tempdir().unwrap();
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let certificate = root.path().join("certificate.pem");
    let key = root.path().join("key.pem");
    std::fs::write(&certificate, cert.pem()).unwrap();
    std::fs::write(&key, signing_key.serialize_pem()).unwrap();
    let mut child = Command::new(ZUSE)
        .args([
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--ipc-port",
            "0",
            "--data-dir",
        ])
        .arg(root.path().join("profile"))
        .arg("--tls-cert")
        .arg(&certificate)
        .arg("--tls-key")
        .arg(&key)
        .env("ZUSE_BROWSER_TOKEN", "b".repeat(64))
        .env_remove("ZUSE_PUBLIC_ORIGIN")
        .env_remove("ZUSE_TLS_CERT")
        .env_remove("ZUSE_TLS_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let origin = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut line = String::new();
            assert!(
                output.read_line(&mut line).await.unwrap() > 0,
                "HTTPS server ended before readiness"
            );
            if let Some(link) = line.trim().strip_prefix("Open ") {
                break link.split("/#token=").next().unwrap().to_owned();
            }
        }
    })
    .await
    .unwrap();
    assert!(origin.starts_with("https://127.0.0.1:"));
    let client = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_der(cert.der()).unwrap())
        .build()
        .unwrap();
    // A valid certificate is required; no certificate-verification bypass is used.
    assert_eq!(
        client
            .get(format!("{origin}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let denied = client
        .post(format!("{origin}/api/session"))
        .header("Origin", "https://foreign.example")
        .json(&json!({"token":"b".repeat(64)}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
    let unlocked = client
        .post(format!("{origin}/api/session"))
        .header("Origin", &origin)
        .json(&json!({"token":"b".repeat(64)}))
        .send()
        .await
        .unwrap();
    assert!(unlocked.status().is_success());
    let cookie = unlocked
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        cookie.contains("; Secure")
            && cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Strict")
    );
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let mut request = format!("{}/api/rpc", origin.replacen("https://", "wss://", 1))
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", origin.parse().unwrap());
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(Connector::Rustls(Arc::new(tls))),
    )
    .await
    .unwrap();
    socket
        .send(Message::Text(
            json!({"id":1,"method":"WatchSessions","params":{}}).to_string(),
        ))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(response["id"], 1);
    assert!(response["item"].is_array());
    // Keep the browser subscription open while the actual process drains.
    let pid = child.id().unwrap();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let closed = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
}

#[tokio::test]
async fn pair_command_creates_links_on_the_running_server_and_says_when_none_runs() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let legacy = legacy_server(root.path());
    let mut server = Server::start(
        &root.path().join("profile"),
        &bin,
        &root.path().join("mcp.json"),
    )
    .await;
    let pair = |program: &Path, label: &str, port: u16| {
        Command::new(program)
            .args(["--language", "en", "pair", "--label", label, "--ipc-port"])
            .arg(port.to_string())
            .kill_on_drop(true)
            .output()
    };
    for (program, label) in [(Path::new(ZUSE), "Phone"), (legacy.as_path(), "Tablet")] {
        let output = pair(program, label, server.port).await.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.status.success(), "{output:?}");
        assert!(stdout.contains("/#pair="), "{stdout}");
        assert!(stdout.contains("only works on this computer"), "{stdout}");
    }
    let pairings = server
        .client()
        .await
        .call(methods::LIST_PAIRINGS, json!({}))
        .await
        .unwrap();
    let mut labels: Vec<_> = pairings
        .as_array()
        .unwrap()
        .iter()
        .map(|pairing| pairing["label"].as_str().unwrap())
        .collect();
    labels.sort();
    assert_eq!(labels, ["Phone", "Tablet"]);
    // `zuse status` adds the running engine's remote access to the local report.
    let status = Command::new(ZUSE)
        .args(["--language", "en", "status", "--ipc-port"])
        .arg(server.port.to_string())
        .env("ZUSE_DATA_DIR", root.path().join("status-profile"))
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(stdout.contains("Mode:"), "{stdout}");
    assert!(stdout.contains("Remote access:"), "{stdout}");
    assert!(stdout.contains("Account: not signed in"), "{stdout}");
    let port = server.port;
    server.stop().await;
    let output = pair(Path::new(ZUSE), "Phone", port).await.unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No Zuse server is running"));
    let status = Command::new(ZUSE)
        .args(["--language", "en", "status", "--ipc-port"])
        .arg(port.to_string())
        .env("ZUSE_DATA_DIR", root.path().join("status-profile"))
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(stdout.contains("IPC:      not listening"), "{stdout}");
    assert!(!stdout.contains("Remote access"), "{stdout}");
}

#[tokio::test]
async fn without_a_command_the_standalone_binary_explains_and_shows_help() {
    let output = Command::new(ZUSE)
        .args(["--language", "en"])
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("desktop app is not installed"), "{stderr}");
    assert!(
        stderr.contains("serve") && stderr.contains("pair"),
        "{stderr}"
    );
}

#[tokio::test]
async fn invalid_network_configuration_fails_before_starting_the_engine() {
    let root = tempfile::tempdir().unwrap();
    let legacy = legacy_server(root.path());
    for (name, arguments, expected) in [
        (
            "unencrypted-lan",
            vec!["--listen", "0.0.0.0:0"],
            "Non-loopback listening requires",
        ),
        (
            "incomplete-tls",
            vec!["--tls-cert", "missing-certificate.pem"],
            "--tls-key",
        ),
    ] {
        // `zuse serve` and the old `zuse-server` refuse alike.
        for (program, serve) in [(Path::new(ZUSE), true), (legacy.as_path(), false)] {
            let profile = root.path().join(format!("{name}-{serve}"));
            let output = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new(program)
                    .args(serve.then_some("serve"))
                    .args(&arguments)
                    .arg("--data-dir")
                    .arg(&profile)
                    .env_remove("ZUSE_TLS_CERT")
                    .env_remove("ZUSE_TLS_KEY")
                    .env_remove("ZUSE_PUBLIC_ORIGIN")
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
            assert!(!profile.exists());
        }
    }
}
