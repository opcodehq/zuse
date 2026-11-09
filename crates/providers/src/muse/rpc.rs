//! The Muse Code host process: `muse serve` speaking newline-delimited
//! JSON-RPC 2.0 over stdio (the Muse Session Protocol, schema version 1).
//!
//! Framing is the same as Codex's app-server, so [`RpcClient`] carries it;
//! this module owns the launch (arguments, environment), the `initialize`
//! handshake and the per-command idempotency ids the protocol requires.
use std::{path::Path, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};

use crate::{
    HarnessError,
    jsonrpc::{Incoming, RpcClient},
    process::{Command, Stdio, owned::Child},
};

/// How the host is launched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Model discovery: no shell, no writes, no session log.
    Probe,
    /// A chat session. `read_only` disables the shell and file writes.
    Session { read_only: bool },
}

/// The command-line arguments after the executable.
pub(super) fn args(mode: Mode) -> Vec<&'static str> {
    match mode {
        Mode::Probe => vec![
            "serve",
            "--disable-shell",
            "--disable-write",
            "--no-session-log",
        ],
        Mode::Session { read_only: false } => vec!["serve", "--trust-workspace"],
        Mode::Session { read_only: true } => vec![
            "serve",
            "--trust-workspace",
            "--disable-shell",
            "--disable-write",
        ],
    }
}

/// Apply the host environment: Zuse owns updates, and the CLI's own login is
/// the credential. An inherited `META_API_KEY` would silently override it.
pub(super) fn environment(command: &mut Command) {
    command.env("MUSE_NO_AUTO_UPDATE", "1");
    command.env_remove("META_API_KEY");
}

pub(super) struct Host {
    pub child: Child,
    pub client: RpcClient,
    pub incoming: mpsc::Receiver<Incoming>,
    pub tail: crate::StderrTail,
    /// The `initialize` result.
    pub initialized: Value,
}

impl Host {
    /// Spawn `muse serve` and complete the handshake within `timeout`.
    pub async fn open(
        exe: &Path,
        cwd: &Path,
        mode: Mode,
        timeout: Duration,
        kill_grace: Duration,
    ) -> Result<Self, HarnessError> {
        let mut cmd = Command::new(exe);
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, exe);
        environment(&mut cmd);
        cmd.args(args(mode))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(exe))
            } else {
                HarnessError::Io(e)
            }
        })?;
        let mut child = Child::new(child);
        let tail = crate::StderrTail::default();
        let mut lines = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
        let stderr = tail.clone();
        // Ends with the host's stderr.
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "zuse_providers::muse", "stderr: {line}");
                stderr.push(&line);
            }
            stderr.close();
        });
        let (client, incoming) = RpcClient::new(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        let mut host = Self {
            child,
            client,
            incoming,
            tail,
            initialized: Value::Null,
        };
        let capabilities = match mode {
            Mode::Probe => json!({ "requestedCapabilities": [] }),
            Mode::Session { .. } => json!({ "requestedCapabilities": ["sessionMcp"] }),
        };
        let handshake = host.client.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "zuse",
                    "title": "Zuse",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": capabilities,
            }),
        );
        let result = match tokio::time::timeout(timeout, handshake).await {
            Ok(Ok(result)) => check_schema(result),
            Ok(Err(error)) => Err(host.exit_error(error).await),
            Err(_) => Err(HarnessError::Protocol(format!(
                "Muse Code did not finish initializing within {}s",
                timeout.as_secs()
            ))),
        };
        match result {
            Ok(result) => {
                host.initialized = result;
                host.client.notify("initialized", None);
                Ok(host)
            }
            Err(error) => {
                host.child.shutdown(kill_grace).await;
                Err(error)
            }
        }
    }

    /// Whether the host granted a capability this connection asked for.
    pub fn granted(&self, capability: &str) -> bool {
        self.initialized["grantedCapabilities"]
            .as_array()
            .is_some_and(|granted| granted.iter().any(|c| c == capability))
    }

    /// A request failed because the host went away: report its exit and
    /// stderr instead of a bare "exited before responding".
    async fn exit_error(&mut self, error: HarnessError) -> HarnessError {
        let status = tokio::time::timeout(Duration::from_millis(300), self.child.wait())
            .await
            .ok()
            .and_then(Result::ok);
        if status.is_some() {
            self.tail.wait_closed().await;
            HarnessError::Protocol(crate::crash_message("Muse Code", status, &self.tail))
        } else {
            error
        }
    }
}

/// Muse speaks schema version 1; anything else is a protocol this client
/// cannot read safely.
pub(super) fn check_schema(result: Value) -> Result<Value, HarnessError> {
    match result.pointer("/schema/version").and_then(Value::as_u64) {
        Some(1) => Ok(result),
        Some(version) => Err(HarnessError::Protocol(format!(
            "Unsupported Muse Code protocol version {version}; update Zuse or Muse Code"
        ))),
        None => Err(HarnessError::Protocol(
            "Unsupported Muse Code protocol version; update Muse Code".into(),
        )),
    }
}

/// A fresh UUIDv7 — the idempotency handle every MSP command carries.
pub(super) fn command_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut bytes = *uuid::Uuid::new_v4().as_bytes();
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    // Version 7; the v4 generator already set the RFC 4122 variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// Send one MSP command (a request carrying a fresh `commandId`).
pub(super) fn command(
    client: &RpcClient,
    method: &str,
    params: Value,
) -> futures::future::BoxFuture<'static, Result<Value, HarnessError>> {
    command_with_id(client, method, params, command_id())
}

/// [`command`] under a caller-minted id (a compaction's turn id is its
/// command id, so the caller needs it before the acknowledgement).
pub(super) fn command_with_id(
    client: &RpcClient,
    method: &str,
    mut params: Value,
    id: String,
) -> futures::future::BoxFuture<'static, Result<Value, HarnessError>> {
    if let Some(object) = params.as_object_mut() {
        object.insert("commandId".into(), Value::String(id));
    }
    client.request_now(method, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_version_7() {
        let a = command_id();
        let parsed = uuid::Uuid::parse_str(&a).unwrap();
        assert_eq!(parsed.get_version_num(), 7);
        assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
        assert_ne!(a, command_id());
    }

    #[test]
    fn schema_version_must_be_one() {
        assert!(check_schema(json!({"schema": {"version": 1}})).is_ok());
        assert!(
            check_schema(json!({"schema": {"version": 2}}))
                .unwrap_err()
                .to_string()
                .contains("version 2")
        );
        assert!(check_schema(json!({})).is_err());
    }

    #[test]
    fn launch_arguments_follow_the_access_level() {
        assert_eq!(
            args(Mode::Probe),
            [
                "serve",
                "--disable-shell",
                "--disable-write",
                "--no-session-log"
            ]
        );
        assert_eq!(
            args(Mode::Session { read_only: false }),
            ["serve", "--trust-workspace"]
        );
        assert_eq!(
            args(Mode::Session { read_only: true }),
            [
                "serve",
                "--trust-workspace",
                "--disable-shell",
                "--disable-write"
            ]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn environment_disables_self_update_and_drops_the_inherited_api_key() {
        let mut command = Command::new("muse");
        environment(&mut command);
        let envs: Vec<_> = command
            .as_std()
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap().to_owned(), v.map(|v| v.to_owned())))
            .collect();
        assert!(envs.contains(&("MUSE_NO_AUTO_UPDATE".into(), Some("1".into()))));
        assert!(envs.contains(&("META_API_KEY".into(), None)));
    }
}
