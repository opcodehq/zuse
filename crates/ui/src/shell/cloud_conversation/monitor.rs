//! A selected computer's existing authenticated SSH transport carries both the
//! viewer handshake and the loopback video stream. Dropping the view kills SSH.
use std::{net::TcpListener, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
};

pub(super) struct Connection {
    _ssh: Child,
    pub(super) url: String,
}

impl Connection {
    pub(super) async fn open(host: &str) -> Result<Self, String> {
        let port = TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map_err(|_| "Could not reserve a local monitor port.")?
            .port();
        let forward = format!("127.0.0.1:{port}:127.0.0.1:6901");
        let mut ssh = Command::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ControlMaster=no",
                "-S",
                "none",
                "-L",
                &forward,
                "--",
                host,
                "/usr/local/bin/zuse-monitor attach",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "Could not start SSH for Monitor.")?;
        let output = ssh
            .stdout
            .take()
            .ok_or("Could not read the monitor handshake.")?;
        let mut token = String::new();
        tokio::time::timeout(
            Duration::from_secs(45),
            BufReader::new(output.take(64)).read_line(&mut token),
        )
        .await
        .map_err(|_| "Monitor timed out while connecting.")?
        .map_err(|_| "Monitor connection failed.")?;
        let token = token.trim_end();
        if token.len() != 43
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(
                "Monitor could not start. Check SSH access and the experimental monitor image."
                    .into(),
            );
        }
        Ok(Self {
            _ssh: ssh,
            url: format!("http://127.0.0.1:{port}/#{token}"),
        })
    }
}
