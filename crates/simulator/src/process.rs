//! Bounded subprocesses; arguments never pass through a host shell.
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
#[async_trait]
pub trait Runner: Send + Sync {
    async fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>>;
    async fn start(&self, program: &str, args: &[String]) -> Result<()>;
}
pub struct SystemRunner;
#[async_trait]
impl Runner for SystemRunner {
    async fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
        tokio::time::timeout(timeout, async {
            let mut child = Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .with_context(|| format!("Could not start {program}"))?;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let read = |pipe: Box<dyn tokio::io::AsyncRead + Unpin + Send>| async move {
                let mut bytes = Vec::new();
                pipe.take(OUTPUT_LIMIT as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                if bytes.len() > OUTPUT_LIMIT {
                    bail!("Device response exceeded 8 MiB");
                }
                Ok::<_, anyhow::Error>(bytes)
            };
            let (stdout, stderr) =
                tokio::try_join!(read(Box::new(stdout)), read(Box::new(stderr)))?;
            let status = child.wait().await?;
            if !status.success() {
                bail!(
                    "{program} failed: {}",
                    String::from_utf8_lossy(&stderr[..stderr.len().min(2000)])
                );
            }
            Ok(stdout)
        })
        .await
        .context("Device command timed out; refresh the device list before retrying")?
    }
    async fn start(&self, program: &str, args: &[String]) -> Result<()> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Could not start {program}"))?;
        // Emulators intentionally outlive the viewer. Reap them when they exit.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Toolchain {
    pub macos: bool,
    pub xcrun: String,
    pub adb: String,
    pub emulator: String,
    pub avdmanager: String,
    pub idb: String,
}
impl Default for Toolchain {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let sdk = std::env::var_os("ANDROID_SDK_ROOT")
            .or_else(|| std::env::var_os("ANDROID_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    home.join("Library/Android/sdk")
                } else if cfg!(windows) {
                    home.join("AppData/Local/Android/Sdk")
                } else {
                    home.join("Android/Sdk")
                }
            });
        let find = |relative: &str, fallback: &str| {
            let path = sdk.join(relative);
            if path.is_file() {
                path.to_string_lossy().into_owned()
            } else {
                fallback.to_owned()
            }
        };
        Self {
            macos: cfg!(target_os = "macos"),
            xcrun: "xcrun".into(),
            adb: find(
                if cfg!(windows) {
                    "platform-tools/adb.exe"
                } else {
                    "platform-tools/adb"
                },
                "adb",
            ),
            emulator: find(
                if cfg!(windows) {
                    "emulator/emulator.exe"
                } else {
                    "emulator/emulator"
                },
                "emulator",
            ),
            avdmanager: find(
                if cfg!(windows) {
                    "cmdline-tools/latest/bin/avdmanager.bat"
                } else {
                    "cmdline-tools/latest/bin/avdmanager"
                },
                "avdmanager",
            ),
            idb: "idb".into(),
        }
    }
}
