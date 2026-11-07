//! Small application client. This binary never starts or links an engine.
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use zuse_protocol::control::{API_VERSION, CALL, METHODS};

#[derive(Parser)]
#[command(version, about = "Control a running local or cloud Zuse engine")]
struct Args {
    /// Private discovery JSON written by the selected engine profile.
    #[arg(long, env = "ZUSE_CONTROL_FILE")]
    discovery: Option<PathBuf>,
    /// Override discovery endpoint; credentials still come from the private file.
    #[arg(long, env = "ZUSE_CONTROL_ENDPOINT")]
    endpoint: Option<String>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// API version, supported capabilities and operation names.
    Info,
    /// Invoke an operation. Use @path to read JSON from a file, or - for stdin.
    Call {
        method: String,
        #[arg(default_value = "{}")]
        json: String,
    },
    /// Print a subscription as newline-delimited JSON until interrupted.
    Watch {
        method: String,
        #[arg(default_value = "{}")]
        json: String,
    },
    /// Serve the application API as MCP tools over stdin/stdout.
    Mcp,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    endpoint: String,
    token: String,
    api_version: u32,
}

fn discovery_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME or --discovery is required")?);
    for root in [".zuse", ".zuse-native"] {
        let path = home.join(root).join("profiles/local/zots/control.json");
        if path.is_file() {
            return Ok(path);
        }
    }
    anyhow::bail!(
        "No running-engine discovery found. Start Zuse or set ZUSE_CONTROL_FILE for the selected profile."
    )
}
fn read_json(input: &str) -> Result<Value> {
    let content = if let Some(path) = input.strip_prefix('@') {
        std::fs::read_to_string(path)?
    } else if input == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        input.into()
    };
    Ok(serde_json::from_str(&content)?)
}
fn envelope(token: &str, method: &str, params: Value) -> Value {
    json!({"token":token,"method":method,"params":params})
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let path = discovery_path(args.discovery)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            std::fs::metadata(&path)?.permissions().mode() & 0o077 == 0,
            "discovery file must be private to its owner"
        );
    }
    let discovery: Discovery = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        discovery.api_version == API_VERSION,
        "unsupported control API version"
    );
    let endpoint = args.endpoint.unwrap_or(discovery.endpoint);
    let parsed = url::Url::parse(&endpoint).context("invalid control endpoint")?;
    ensure!(
        parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none(),
        "endpoint must not contain credentials, queries or fragments"
    );
    ensure!(
        parsed.scheme() == "wss"
            || (parsed.scheme() == "ws"
                && matches!(
                    parsed.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                )),
        "remote control requires TLS"
    );
    ensure!(
        endpoint.starts_with("wss://")
            || endpoint.starts_with("ws://127.0.0.1:")
            || endpoint.starts_with("ws://localhost:"),
        "remote control requires wss://"
    );
    let client = zuse_rpc::connect_ws(&endpoint).await?;
    let info = client
        .call_scoped(CALL, envelope(&discovery.token, "ControlInfo", json!({})))
        .await?;
    ensure!(
        info.get("apiVersion").and_then(Value::as_u64) == Some(API_VERSION.into()),
        "incompatible engine API"
    );
    match args.command {
        Command::Info => println!("{}", serde_json::to_string_pretty(&info)?),
        Command::Call {
            method,
            json: input,
        } => {
            ensure!(
                METHODS.contains(&method.as_str()),
                "unsupported operation; run info for supported methods"
            );
            let result = client
                .call_scoped(
                    CALL,
                    envelope(&discovery.token, &method, read_json(&input)?),
                )
                .await?;
            println!("{}", serde_json::to_string(&result)?);
        }
        Command::Watch {
            method,
            json: input,
        } => {
            ensure!(METHODS.contains(&method.as_str()), "unsupported operation");
            let mut stream = client
                .subscribe_checked(
                    CALL,
                    envelope(&discovery.token, &method, read_json(&input)?),
                )
                .await?;
            loop {
                tokio::select! {
                    item = stream.recv() => match item { Some(item) => println!("{}", serde_json::to_string(&item)?), None => break },
                    _ = tokio::signal::ctrl_c() => break,
                }
            }
        }
        Command::Mcp => serve_mcp(client, discovery.token).await?,
    }
    Ok(())
}

async fn serve_mcp(client: zuse_rpc::RpcClient, token: String) -> Result<()> {
    let client = std::sync::Arc::new(client);
    let token = std::sync::Arc::new(token);
    let (out, mut replies) = tokio::sync::mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(value) = replies.recv().await {
            if stdout
                .write_all(format!("{value}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
            let _ = stdout.flush().await;
        }
    });
    let mut tasks = std::collections::HashMap::new();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                out.send(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON"}})).await?;
                continue;
            }
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if method == "notifications/cancelled" {
            if let Some(id) = request.pointer("/params/requestId") {
                if let Some(task) = tasks.remove(&id.to_string()) {
                    let task: tokio::task::JoinHandle<()> = task;
                    task.abort();
                }
            }
            continue;
        }
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        tasks.retain(|_, task| !task.is_finished());
        if tasks.len() >= 64 {
            out.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"Too many active requests"}})).await?;
            continue;
        }
        let params = request.get("params").cloned().unwrap_or(json!({}));
        let (client, token, out) = (client.clone(), token.clone(), out.clone());
        let key = id.to_string();
        if tasks.contains_key(&key) {
            continue;
        }
        tasks.insert(key, tokio::spawn(async move {
            let response = match method.as_str() {
                "initialize" => json!({"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"zuse-control","version":env!("CARGO_PKG_VERSION")}}}),
                "ping" => json!({"result":{}}),
                "tools/list" => json!({"result":{"tools":METHODS.iter().filter(|m| !m.starts_with("Watch") && **m != "SubscribeTerminal").map(|method| json!({"name":method,"description":format!("Zuse application operation {method}"),"inputSchema":if *method=="ZotCommand" {zuse_protocol::zots::command_schema()} else {json!({"type":"object","additionalProperties":true})}})).collect::<Vec<_>>()}}),
                "tools/call" => {
                    let method = params.get("name").and_then(Value::as_str).unwrap_or_default();
                    let result = client.call_scoped(CALL, envelope(&token, method, params.get("arguments").cloned().unwrap_or(json!({})))).await;
                    match result {
                        Ok(value) => json!({"result":{"content":[{"type":"text","text":value.to_string()}]}}),
                        Err(error) => json!({"result":{"isError":true,"content":[{"type":"text","text":error.to_string()}]}}),
                    }
                }
                _ => json!({"error":{"code":-32601,"message":"Unknown method"}}),
            };
            let mut response = response;
            response["jsonrpc"] = json!("2.0"); response["id"] = id;
            let _ = out.send(response).await;
        }));
    }
    for task in tasks.into_values() {
        task.abort();
    }
    drop(out);
    writer.await?;
    Ok(())
}
