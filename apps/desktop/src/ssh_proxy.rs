//! `zuse ssh-proxy <host>`: the ProxyCommand behind `ssh zuse-<workspace>`.
//!
//! It asks the local engine for the workspace's computer (waking it when it
//! sleeps), opens a tunnel to the computer's loopback sshd through the same
//! cloud connection the app uses, and pipes stdin and stdout through it.
use anyhow::{Context as _, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zuse_rpc::methods;

/// Host aliases are `zuse-<workspace id>`.
pub fn workspace_from_host(host: &str) -> Option<&str> {
    host.strip_prefix("zuse-")
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

/// One payload of a `CloudRpc` reply stream: each item is the computer's own
/// reply frame.
fn payload(frame: &Value) -> Result<Option<Value>> {
    if let Some(error) = frame["err"].as_str() {
        bail!("{error}");
    }
    if frame.get("item").is_some() {
        return Ok(Some(frame["item"].clone()));
    }
    if frame.get("ok").is_some() {
        return Ok(Some(frame["ok"].clone()));
    }
    Ok(None)
}

struct Scope {
    workspace: String,
    subject: String,
    organization: Option<String>,
}

impl Scope {
    fn rpc(&self, request: u64, method: &str, params: Value) -> Value {
        json!({
            "workspaceId": self.workspace,
            "expectedAccountId": self.subject,
            "expectedOrganizationId": self.organization,
            "logicalCommandId": uuid::Uuid::new_v4().to_string(),
            "requestId": request,
            "method": method,
            "params": params,
        })
    }
}

async fn unary(client: &zuse_rpc::RpcClient, scope: &Scope, request: u64, method: &str, params: Value) -> Result<Value> {
    let mut reply = client
        .subscribe_scoped(methods::CLOUD_RPC, scope.rpc(request, method, params))
        .await?;
    while let Some(frame) = reply.recv().await {
        if let Some(value) = payload(&frame)? {
            return Ok(value);
        }
    }
    Err(anyhow!("The computer closed the request."))
}

pub async fn run(ipc_port: u16, host: &str) -> Result<()> {
    let workspace = workspace_from_host(host)
        .with_context(|| format!("{host} is not a Zuse cloud host (expected zuse-<workspace>)"))?
        .to_owned();
    let client = std::sync::Arc::new(
        zuse_rpc::connect_ws(&format!("ws://127.0.0.1:{ipc_port}"))
            .await
            .map_err(|e| anyhow!("Zuse isn't running on this computer ({e}). Open Zuse, then retry."))?,
    );
    let mut account = client
        .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
        .await?;
    let status = account.recv().await.context("No account status from Zuse.")?;
    let subject = status["user"]["id"]
        .as_str()
        .context("Sign in to Zuse to reach cloud computers.")?
        .to_owned();
    let scope = Scope {
        workspace: workspace.clone(),
        subject: subject.clone(),
        organization: status["orgId"].as_str().map(str::to_owned),
    };
    // Held for the whole session: it keeps the computer's connection open,
    // and waking it is the point of connecting.
    let mut environment = client
        .subscribe_scoped(
            methods::CLOUD_ENVIRONMENT,
            json!({"workspaceId": workspace, "expectedAccountId": subject,
                   "expectedOrganizationId": scope.organization, "interactiveResume": true}),
        )
        .await?;
    loop {
        let value = environment
            .recv()
            .await
            .context("The computer's connection closed.")?;
        match value["state"].as_str() {
            Some("ready") => break,
            Some("blocked" | "failed") => bail!(
                "{}",
                value["message"].as_str().unwrap_or("The computer could not start.")
            ),
            _ => {}
        }
    }
    let mut tunnel_stream = client
        .subscribe_scoped(methods::CLOUD_RPC, scope.rpc(1, methods::SSH_TUNNEL, json!({})))
        .await?;
    let tunnel = loop {
        let frame = tunnel_stream.recv().await.context("The computer refused SSH.")?;
        if let Some(value) = payload(&frame)?
            && let Some(id) = value["tunnel"].as_str()
        {
            break id.to_owned();
        }
    };
    let upstream = {
        let client = client.clone();
        let tunnel = tunnel.clone();
        let scope = Scope {
            workspace: scope.workspace.clone(),
            subject: scope.subject.clone(),
            organization: scope.organization.clone(),
        };
        tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buffer = vec![0u8; 32 * 1024];
            let mut request = 2u64;
            loop {
                let n = match stdin.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                request += 1;
                let data = STANDARD.encode(&buffer[..n]);
                if unary(&client, &scope, request, methods::SSH_TUNNEL_WRITE, json!({"tunnel": tunnel, "data": data}))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            let _ = unary(&client, &scope, request + 1, methods::SSH_TUNNEL_CLOSE, json!({"tunnel": tunnel})).await;
        })
    };
    let mut stdout = tokio::io::stdout();
    while let Some(frame) = tunnel_stream.recv().await {
        let Some(value) = payload(&frame)? else {
            if frame["done"] == true {
                break;
            }
            continue;
        };
        if let Some(data) = value["data"].as_str() {
            let bytes = STANDARD.decode(data).context("Invalid data from the computer.")?;
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
    }
    upstream.abort();
    drop(environment);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_name_one_workspace() {
        assert_eq!(workspace_from_host("zuse-workspace_123"), Some("workspace_123"));
        assert_eq!(workspace_from_host("zuse-"), None);
        assert_eq!(workspace_from_host("other"), None);
        assert_eq!(workspace_from_host("zuse-a;rm -rf"), None);
    }

    #[test]
    fn reply_frames_unwrap_to_their_payload() {
        assert_eq!(payload(&json!({"id":1,"item":{"tunnel":"t"}})).unwrap(), Some(json!({"tunnel":"t"})));
        assert_eq!(payload(&json!({"id":1,"ok":{"ok":true}})).unwrap(), Some(json!({"ok":true})));
        assert!(payload(&json!({"id":1,"err":"nope"})).is_err());
        assert_eq!(payload(&json!({"id":1,"done":true})).unwrap(), None);
    }
}
