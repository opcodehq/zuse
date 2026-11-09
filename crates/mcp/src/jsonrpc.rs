//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! Requests are handled concurrently (a `wait_for_turn` may block for
//! minutes while the client keeps pinging); responses are serialized through
//! one writer task so frames never interleave. Notifications from the client
//! initialize the session or cancel their matching in-flight request. Dropping
//! an external tool request also cancels its outstanding engine RPC.

use std::{collections::HashMap, sync::Arc};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::tools::Tools;

/// Protocol revisions this server knows. The client's requested revision
/// is echoed when it is one of these; otherwise the newest is offered and
/// the client decides whether to continue (per the MCP handshake rules).
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "\
Zuse runs coding agents in chats, each hosted on a device, optionally in a project \
(a folder on that specific device). These tools operate the local Zuse engine: \
discover devices/projects/chats, create chats with a chosen harness and \
model, read transcripts, and send messages between chats.\n\
\n\
Chats are referenced by full id, a unique id prefix, or an exact title. Use \
`whoami` to learn which chat you are speaking from; messages you send are \
attributed to it and a chat cannot message itself. For request/response \
with a single chat, `send_message` with `wait: true` returns its reply. \
For parallel delegation, use `create_chats` with a prompt for each chat or \
`send_messages` for existing chats. These batch tools launch requests concurrently \
even if your harness executes tool calls sequentially. Alternatively, launch ALL \
chats/messages with `wait: false` first, then use `wait_for_turn` to collect replies. \
Do not wait for one worker before launching the next independent worker. \
Use `kind: chat` in `create_chat` or each `create_chats` request for a standalone \
session visible in Sessions; it cannot have a parent. Use `kind: side` for a child \
with an explicit parent or your origin chat. Omitted kind keeps legacy defaults: \
side with an origin/parent, standalone otherwise. Side chats cannot create chats. \
Discover hosts with `list_devices`, then `list_projects {device}`, \
`list_harnesses {device}` and `list_models {device, harness}`. A project fixes \
its host; with both project and device it must belong to that device. Use listed \
ids when names/paths repeat. Without project or device, creation uses the local engine. \
If a chat is `awaitingInput`, answer it with `respond_to_input`.\n\
\n\
When `show_ui` is available, use it unprompted for structured answers: a final answer with \
several changes, checks, findings, or options gets one compact UI block (charts, tables, steps, \
flows, key-value checks) as its main summary, shown below your reply. Simple answers stay text. \
Use `show_visual` (a Luau script) only for custom drawing, 3D scenes, shaders, or live \
simulations the components can't express. Never announce or restate what a visual shows.";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serve MCP on this process's stdin/stdout until stdin closes.
pub async fn serve_stdio(tools: Arc<Tools>) -> anyhow::Result<()> {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err()
                || stdout.write_all(b"\n").await.is_err()
                || stdout.flush().await.is_err()
            {
                break;
            }
        }
    });

    let mut running = HashMap::new();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                let _ = out_tx
                    .send(
                        error_response(Value::Null, PARSE_ERROR, &format!("parse error: {err}"))
                            .to_string(),
                    )
                    .await;
                continue;
            }
        };
        let Some(response) = route(&tools, message, &out_tx, &mut running) else {
            continue;
        };
        let _ = out_tx.send(response.to_string()).await;
    }
    for (_, task) in running {
        task.abort();
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// Classify one inbound message. Requests are answered asynchronously via
/// `out`; an immediate `Some` is a synchronous error reply.
fn route(
    tools: &Arc<Tools>,
    message: Value,
    out: &mpsc::Sender<String>,
    running: &mut HashMap<String, tokio::task::AbortHandle>,
) -> Option<Value> {
    running.retain(|_, task| !task.is_finished());
    let Some(object) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC object",
        ));
    };
    let id = object.get("id").cloned();
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => {
            let key = id.to_string();
            if running.contains_key(&key) || running.len() >= 128 {
                return Some(error_response(
                    id,
                    INVALID_REQUEST,
                    "Request is already running or the concurrency limit was reached.",
                ));
            }
            let tools = tools.clone();
            let out = out.clone();
            let task = tokio::spawn(async move {
                let response = handle_request(&tools, id, &method, params).await;
                let _ = out.send(response.to_string()).await;
            });
            running.insert(key, task.abort_handle());
            None
        }
        (None, Some(method)) => {
            if method == "notifications/cancelled" {
                if let Some(id) = params.get("requestId") {
                    if let Some(task) = running.remove(&id.to_string()) {
                        task.abort();
                    }
                }
            }
            tracing::debug!(method, "mcp: notification");
            None
        }
        // A response to a server-initiated request: we never send any.
        (Some(_), None) => None,
        (None, None) => Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "missing method",
        )),
    }
}

/// Answer one request. Public for tests and for embedding the dispatcher
/// behind another transport.
pub async fn handle_request(tools: &Tools, id: Value, method: &str, params: Value) -> Value {
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST_PROTOCOL);
            let version = if PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                LATEST_PROTOCOL
            };
            ok_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "zuse",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tools.list() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return error_response(id, INVALID_PARAMS, "tools/call needs a tool name");
            };
            if !tools.has(name) {
                return error_response(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tools.call(name, arguments).await {
                Ok(value) => ok_response(
                    id,
                    json!({
                        "content": crate::simulators::image_content(name, &value).map(|image| vec![json!({"type":"text","text":format!("{} × {} pixels",value["width"],value["height"])}),image]).unwrap_or_else(||vec![json!({ "type": "text", "text": pretty(&value) })]),
                        "structuredContent": wrap_structured(if name == "device_screenshot" { json!({"width":value["width"],"height":value["height"],"mimeType":"image/png"}) } else {value}),
                        "isError": false,
                    }),
                ),
                Err(message) => ok_response(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true,
                    }),
                ),
            }
        }
        // Optional capabilities we do not advertise; answer empty rather
        // than erroring so permissive clients do not log noise.
        "resources/list" => ok_response(id, json!({ "resources": [] })),
        "prompts/list" => ok_response(id, json!({ "prompts": [] })),
        other => error_response(id, METHOD_NOT_FOUND, &format!("unknown method: {other}")),
    }
}

/// `structuredContent` must be an object; arrays and scalars are wrapped.
fn wrap_structured(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({ "result": value })
    }
}

fn pretty(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::{Origin, Zuse};
    use tokio::sync::Notify;
    use zuse_rpc::{RpcError, RpcReply, RpcService, memory_client};
    struct Blocking {
        started: Notify,
        ended: Arc<Notify>,
    }
    struct Ended(Arc<Notify>);
    impl Drop for Ended {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    #[async_trait::async_trait]
    impl RpcService for Blocking {
        async fn handle(&self, _: &str, params: Value) -> Result<RpcReply, RpcError> {
            if params["request"]["action"] == "list" {
                return Ok(RpcReply::Value(json!([])));
            }
            let _ended = Ended(self.ended.clone());
            self.started.notify_one();
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn plugin_cancellation_reaches_engine_without_closing_shared_client() {
        let ended = Arc::new(Notify::new());
        let service = Arc::new(Blocking {
            started: Notify::new(),
            ended: ended.clone(),
        });
        let tools = Arc::new(Tools::new(Arc::new(Zuse::with_client(
            memory_client(service.clone()),
            Origin {
                chat_id: Some("chat".into()),
                device_id: None,
            },
        ))));
        let (out, mut replies) = mpsc::channel(4);
        let mut running = HashMap::new();
        assert!(route(&tools,json!({"id":7,"method":"tools/call","params":{"name":"plugins_call","arguments":{"address":"test/run","arguments":{}}}}),&out,&mut running).is_none());
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            service.started.notified(),
        )
        .await
        .unwrap();
        route(
            &tools,
            json!({"method":"notifications/cancelled","params":{"requestId":7}}),
            &out,
            &mut running,
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), ended.notified())
            .await
            .unwrap();
        assert!(running.is_empty());
        assert!(replies.try_recv().is_err());
        // The cancelled request must leave this long-lived MCP client usable.
        route(
            &tools,
            json!({"id":8,"method":"tools/call","params":{"name":"plugins_list","arguments":{}}}),
            &out,
            &mut running,
        );
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), replies.recv())
            .await
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], 8);
        assert_ne!(response["result"]["isError"], true);
    }
}
