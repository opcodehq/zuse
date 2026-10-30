//! Origin-bound agent CLI sessions; no ambient daemon, target or config is accepted.
use crate::{
    Device, Platform, Target,
    setup::{AGENT, Process, private_dir},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Lease {
    process: Process,
    port: u16,
    token: String,
}
struct Session {
    root: PathBuf,
    cancelled: tokio::sync::watch::Sender<bool>,
    lease: Mutex<Option<Lease>>,
    operation: tokio::sync::Mutex<()>,
}
impl Session {
    fn stop(&self) {
        self.cancelled.send_replace(true);
        self.lease.lock().unwrap().take();
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}
#[derive(Default)]
struct Sessions {
    allowed: HashSet<String>,
    devices: HashMap<(String, String), Arc<Session>>,
}
pub(crate) struct Agents {
    root: PathBuf,
    tools: crate::Toolchain,
    sessions: Mutex<Sessions>,
}
impl Agents {
    pub fn new(root: PathBuf, tools: crate::Toolchain) -> Self {
        Self {
            root,
            tools,
            sessions: Mutex::new(Sessions::default()),
        }
    }
    pub fn allow(&self, chat: &str) {
        self.sessions
            .lock()
            .unwrap()
            .allowed
            .insert(chat.to_owned());
    }
    pub fn revoke_chat(&self, chat: &str) {
        let mut store = self.sessions.lock().unwrap();
        store.allowed.remove(chat);
        store.devices.retain(|(owner, _), s| {
            if owner == chat {
                s.stop();
                false
            } else {
                true
            }
        });
    }
    pub fn close_chat(&self, chat: &str) {
        self.sessions
            .lock()
            .unwrap()
            .devices
            .retain(|(owner, _), s| {
                if owner == chat {
                    s.stop();
                    false
                } else {
                    true
                }
            });
    }
    pub fn close(&self, target: &Target) {
        if let Some(s) = self
            .sessions
            .lock()
            .unwrap()
            .devices
            .remove(&(target.chat_id.clone(), target.device_id.clone()))
        {
            s.stop();
        }
    }
    pub fn retain(&self, chats: &HashSet<String>) {
        let mut store = self.sessions.lock().unwrap();
        store.allowed.retain(|id| chats.contains(id));
        store.devices.retain(|(owner, _), s| {
            if !chats.contains(owner) {
                s.stop();
                false
            } else {
                true
            }
        });
    }
    pub fn shutdown(&self) {
        let mut store = self.sessions.lock().unwrap();
        store.allowed.clear();
        for (_, s) in store.devices.drain() {
            s.stop();
        }
    }
    pub async fn prepare(&self, target: &Target, device: &Device) -> Result<()> {
        self.execute(target, device, None).await
    }
    pub async fn input(&self, target: &Target, device: &Device, args: Vec<String>) -> Result<()> {
        self.execute(target, device, Some(args)).await
    }
    async fn execute(
        &self,
        target: &Target,
        device: &Device,
        args: Option<Vec<String>>,
    ) -> Result<()> {
        let key = (target.chat_id.clone(), target.device_id.clone());
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&key)?));
        let state = self.root.join("sessions").join(&digest);
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            ensure!(
                sessions.allowed.contains(&target.chat_id),
                "Agent device access was revoked"
            );
            sessions
                .devices
                .entry(key)
                .or_insert_with(|| {
                    Arc::new(Session {
                        root: state,
                        cancelled: tokio::sync::watch::channel(false).0,
                        lease: Mutex::new(None),
                        operation: tokio::sync::Mutex::new(()),
                    })
                })
                .clone()
        };
        let mut cancel = session.cancelled.subscribe();
        let observed = session.cancelled.subscribe();
        ensure!(!*observed.borrow(), "Agent device access was revoked");
        let operation = async {
            let _operation = session.operation.lock().await;
            ensure!(!*observed.borrow(), "Agent device access was revoked");
            let mut alive = false;
            if let Some(lease) = session.lease.lock().unwrap().as_mut() {
                alive = lease.process.alive()?;
            }
            if !alive {
                session.lease.lock().unwrap().take();
                private_dir(&session.root)?;
                let registration = session.root.join("daemon.json");
                if registration.exists() {
                    tokio::fs::remove_file(&registration).await?;
                }
                let config = session.root.join("config.json");
                tokio::fs::write(&config, b"{}").await?;
                let entry = AGENT.entry(&self.root);
                ensure!(
                    entry.is_file(),
                    "Finish installing agent tools in the Simulator pane"
                );
                let daemon = entry
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("dist/src/internal/daemon.js");
                let mut command = crate::process::node_command();
                crate::process::sdk_environment(&mut command, &self.tools);
                command
                    .arg(daemon)
                    .env("AGENT_DEVICE_STATE_DIR", &session.root)
                    .env("AGENT_DEVICE_CONFIG", &config)
                    .env("AGENT_DEVICE_DAEMON_SERVER_MODE", "http")
                    .env("AGENT_DEVICE_IOS_RUNNER_DETACH", "false")
                    .env("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1");
                let mut process = Process::start(&mut command)?;
                let registration = tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        ensure!(process.alive()?, "Agent helper exited during startup");
                        if let Ok(bytes) = tokio::fs::read(&registration).await {
                            if bytes.len() < 16384 {
                                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                                    if value["pid"].as_u64() == process.id().map(u64::from) {
                                        return Ok::<_, anyhow::Error>(value);
                                    }
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                })
                .await
                .context("Agent helper startup timed out")??;
                let port = registration["httpPort"]
                    .as_u64()
                    .filter(|p| *p > 0 && *p <= 65535)
                    .context("Invalid owned agent port")? as u16;
                let token = registration["token"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() < 1024)
                    .context("Invalid owned agent token")?
                    .to_owned();
                ensure!(!*observed.borrow(), "Agent device access was revoked");
                *session.lease.lock().unwrap() = Some(Lease {
                    process,
                    port,
                    token,
                });
            }
            let Some(args) = args else {
                return Ok(());
            };
            let (port, token) = {
                let lease = session.lease.lock().unwrap();
                let lease = lease.as_ref().context("Agent device session closed")?;
                (lease.port, lease.token.clone())
            };
            let mut command = crate::process::node_command();
            crate::process::sdk_environment(&mut command, &self.tools);
            command
                .arg(AGENT.entry(&self.root))
                .args(target_args(device, &session.root, &digest)?)
                .args(args)
                .env(
                    "AGENT_DEVICE_DAEMON_BASE_URL",
                    format!("http://127.0.0.1:{port}"),
                )
                .env("AGENT_DEVICE_DAEMON_AUTH_TOKEN", token)
                .env("AGENT_DEVICE_DAEMON_TRANSPORT", "http")
                .env("AGENT_DEVICE_IOS_RUNNER_DETACH", "false")
                .env("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1");
            crate::process::output(command, Duration::from_secs(240))
                .await
                .context("Agent device input failed")?;
            Ok(())
        };
        // Access revocation terminates startup, CLI and owned daemon descendants.
        tokio::select! {biased;
            _=cancel.changed()=>bail!("Agent device access was revoked"),
            result=operation=>result,
        }
    }
}
impl Drop for Agents {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn target_args(device: &Device, root: &std::path::Path, session: &str) -> Result<Vec<String>> {
    let (platform, flag, id) = match device.platform {
        Platform::Ios => ("ios", "--udid", device.id.as_str()),
        Platform::Android => (
            "android",
            "--serial",
            device.serial.as_deref().context("Emulator has no serial")?,
        ),
    };
    Ok(vec![
        "--config".into(),
        root.join("config.json").to_string_lossy().into_owned(),
        "--platform".into(),
        platform.into(),
        flag.into(),
        id.into(),
        "--session".into(),
        format!("zuse-{session}"),
        "--state-dir".into(),
        root.to_string_lossy().into_owned(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn revoked_consent_cannot_allocate_a_late_daemon() {
        let root = tempfile::tempdir().unwrap();
        let agents = Agents::new(root.path().into(), crate::Toolchain::default());
        let target = Target {
            chat_id: "chat".into(),
            device_id: "android:Pixel".into(),
        };
        let device = Device {
            id: target.device_id.clone(),
            name: "Pixel".into(),
            platform: Platform::Android,
            runtime: "Android".into(),
            booted: true,
            serial: Some("emulator-5554".into()),
        };
        assert!(agents.prepare(&target, &device).await.is_err());
        agents.allow("chat");
        agents.revoke_chat("chat");
        assert!(agents.prepare(&target, &device).await.is_err());
        assert!(!root.path().join("sessions").exists());
    }
    #[test]
    fn cli_target_is_bound_and_ambient_configuration_is_not_used() {
        let device = Device {
            id: "android:Pixel".into(),
            name: "Pixel".into(),
            platform: Platform::Android,
            runtime: "Android".into(),
            booted: true,
            serial: Some("emulator-5554".into()),
        };
        let args = target_args(&device, std::path::Path::new("/private/session"), "bound").unwrap();
        assert!(args.windows(2).any(|p| p == ["--serial", "emulator-5554"]));
        assert!(
            args.windows(2)
                .any(|p| p == ["--config", "/private/session/config.json"])
        );
        assert!(args.windows(2).any(|p| p == ["--session", "zuse-bound"]));
        let mut device = device;
        device.serial = None;
        assert!(target_args(&device, std::path::Path::new("/private/session"), "bound").is_err());
    }
}
