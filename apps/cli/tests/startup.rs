#![cfg(unix)]
//! Real server/agent boundary: independent ephemeral listeners and graceful exit.
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout, Command},
};
use zuse_rpc::{RpcClient, connect_ws, methods};

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
        let mut child = Command::new(env!("CARGO_BIN_EXE_zuse-server"))
            .args(["--listen", "127.0.0.1:0", "--ipc-port", "0", "--data-dir"])
            .current_dir(profile.parent().unwrap())
            .arg(profile.file_name().unwrap())
            .env("PATH", path)
            .env("CLAUDE_CODE_EXECUTABLE", bin.join("claude"))
            .env("ZUSE_STARTUP_CAPTURE", capture)
            .env("ZUSE_STARTUP_TOOL_FRAME", bin.join("tool-result.json"))
            .env(
                "ZUSE_BROWSER_TOKEN",
                "local-startup-fixture-token-with-32-characters",
            )
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
    first.stop().await;
    second.stop().await;
    let mut reopened = Server::start(&root.path().join("one"), &bin, &capture).await;
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
