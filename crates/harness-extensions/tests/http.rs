#![cfg(unix)]

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
use zuse_harness::{Error, PermissionMode, ToolContext, ToolOutput};
use zuse_harness_extensions::Registry;
async fn fixture() -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let captured = seen.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let seen = captured.clone();
            tokio::spawn(async move {
                let mut data = vec![];
                let mut chunk = [0; 4096];
                let boundary = loop {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    if data.len() > 64 * 1024 {
                        return;
                    }
                    if let Some(i) = data.windows(4).position(|b| b == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&data[..boundary]).to_ascii_lowercase();
                seen.lock().unwrap().push(headers.clone());
                if headers.starts_with("get ") {
                    let _=socket.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let len = headers
                    .lines()
                    .find_map(|s| {
                        s.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while data.len() - boundary < len {
                    let n = socket.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&chunk[..n]);
                }
                let Ok(p) = serde_json::from_slice::<Value>(&data[boundary..boundary + len]) else {
                    return;
                };
                if p.get("id").is_none() {
                    let _=socket.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let method = p["method"].as_str().unwrap_or("");
                if method == "tools/call" && p["params"]["name"] == "disconnect" {
                    return;
                }
                let result = match method {
                    "server/discover" => serde_json::to_value(rmcp::model::DiscoverResult::new(
                        vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                        serde_json::from_value(json!({"tools":{}})).unwrap(),
                    ))
                    .unwrap(),
                    "initialize" => {
                        json!({"protocolVersion":p["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}})
                    }
                    "tools/list" => {
                        json!({"tools":[{"name":"echo","description":"fixture","inputSchema":{"type":"object"}}]})
                    }
                    _ => json!({"content":[{"type":"text","text":"hello"}],"isError":false}),
                };
                let response = json!({"jsonrpc":"2.0","id":p["id"],"result":result});
                let sse = method == "tools/call";
                let body = if sse {
                    format!("event: message\ndata: {response}\n\n")
                } else {
                    response.to_string()
                };
                let kind = if sse {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                for part in body.as_bytes().chunks(7) {
                    if socket.write_all(part).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (url, seen, task)
}
fn context() -> ToolContext {
    ToolContext {
        agent_id: "a".into(),
        call_id: "call".into(),
        cancel: CancellationToken::new(),
        permission_mode: PermissionMode::Default,
        read_only: false,
    }
}
#[tokio::test]
async fn modern_and_legacy_http_use_one_pool_and_handle_fragmented_sse_without_replaying_calls() {
    for version in ["2025-06-18", "2026-07-28"] {
        let (url, seen, task) = fixture().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("harness")).unwrap();
        std::fs::write(root.path().join("harness/extensions.json"),json!({"servers":{"fixture":{"trusted":true,"protocol":version,"connection":{"type":"streamable-http","url":url}}}}).to_string()).unwrap();
        let registry = Registry::load(root.path()).unwrap();
        let scope = registry
            .scope(root.path().into(), Arc::new(|| "default".into()))
            .unwrap();
        let tools = scope.tools("fixture", None, context()).await.unwrap();
        assert_eq!(tools["tools"][0]["name"], "echo");
        let ToolOutput::Text(result) = scope
            .call("fixture", "echo", json!({}), context())
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(result.contains("hello"));
        assert!(matches!(
            scope
                .call("fixture", "disconnect", json!({}), context())
                .await,
            Err(Error::Paused(_))
        ));
        let headers = seen.lock().unwrap().clone();
        if version == "2026-07-28" {
            assert!(
                headers
                    .iter()
                    .any(|s| s.contains("mcp-method: tools/call") && s.contains("mcp-name: echo"))
            );
            assert!(!headers.iter().any(|s| s.starts_with("get ")));
        }
        assert_eq!(
            headers
                .iter()
                .filter(|s| s.contains("mcp-name: disconnect"))
                .count(),
            if version == "2026-07-28" { 1 } else { 0 }
        );
        task.abort();
    }
}
