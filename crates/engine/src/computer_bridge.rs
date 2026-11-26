//! Cloud-chat enrollment uses a dedicated profile key. A Remote Access
//! invitation and its host key never participate in this protocol.
use anyhow::{Context, Result, ensure};
use p256::ecdsa::SigningKey;
use std::{
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

fn signing_key(profile: &Path) -> Result<SigningKey> {
    let dir = profile.join("computer-bridge");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join("proof.key");
    if !path.try_exists()? {
        let secret = SigningKey::random(&mut rand_core::OsRng);
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        tmp.write_all(&Zeroizing::new(secret.to_bytes())[..])?;
        tmp.as_file().sync_all()?;
        match tmp.persist_noclobber(&path) {
            Ok(_) => (),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.error.into()),
        }
    }
    let file = std::fs::File::open(&path)?;
    ensure!(file.metadata()?.is_file(), "Invalid computer bridge key");
    let mut data = Zeroizing::new(Vec::new());
    file.take(33).read_to_end(&mut data)?;
    ensure!(data.len() == 32, "Invalid computer bridge key");
    SigningKey::from_slice(&data).context("Invalid computer bridge key")
}

pub async fn enroll(profile: &Path, label: &str) -> Result<String> {
    std::fs::create_dir_all(profile)?;
    let _profile_lock = crate::InstanceLock::acquire(profile)?;
    let account = crate::account::Account::open(profile)?;
    enroll_account(profile, label, &account).await
}
async fn enroll_account(
    profile: &Path,
    label: &str,
    account: &crate::account::Account,
) -> Result<String> {
    let label = label.trim();
    ensure!(
        !label.is_empty() && label.len() <= 128 && !label.chars().any(char::is_control),
        "Invalid computer name"
    );
    let origin = zuse_remote::normalized_origin(&crate::cloud::configured_api_origin())?;
    let url = format!("{}/v1/cloud/bridge/computers", origin.trim_end_matches('/'));

    let credential = account
        .credential()
        .await
        .context("Sign in to your Zuse account to enroll a computer")?;
    let key = crate::managed_access::proof::ProofKey::from_signing_key(signing_key(profile)?)?;
    let proof = key.sign("POST", &url, Some(&credential.access_token))?;
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()?
        .post(url)
        .bearer_auth(credential.access_token)
        .header("dpop", proof)
        .json(&serde_json::json!({"label":label,"platform":std::env::consts::OS}))
        .send()
        .await?
        .error_for_status()?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Enrolled {
        device_id: String,
    }
    Ok(response.json::<Enrolled>().await?.device_id)
}

/// Only the foreground operator supplies argv and cwd. Network replies carry a request UUID.
pub async fn serve_task(profile: &Path, argv: Vec<String>) -> Result<()> {
    serve(profile, argv, false, None).await
}
pub async fn serve_computer(profile: &Path) -> Result<()> {
    serve_computer_account(profile, None).await
}
async fn serve_computer_account(
    profile: &Path,
    account: Option<crate::account::Account>,
) -> Result<()> {
    #[cfg(windows)]
    let shell =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").context("Missing SystemRoot")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    #[cfg(not(windows))]
    let shell = std::path::PathBuf::from("/bin/sh");
    serve(
        profile,
        vec![shell.to_str().context("Unsupported shell path")?.to_owned()],
        true,
        account,
    )
    .await
}
async fn serve(
    profile: &Path,
    mut argv: Vec<String>,
    dynamic: bool,
    account: Option<crate::account::Account>,
) -> Result<()> {
    use serde_json::{Value, json};
    ensure!(
        !argv.is_empty(),
        "Specify an absolute executable path and its arguments"
    );
    let executable = Path::new(&argv[0]);
    ensure!(
        executable.is_absolute() && executable.is_file(),
        "Specify an absolute executable path"
    );
    argv[0] = executable
        .canonicalize()?
        .to_str()
        .context("Unsupported executable path")?
        .to_owned();
    let cwd = if dynamic {
        std::path::PathBuf::from(
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .context("Missing home directory")?,
        )
        .canonicalize()?
    } else {
        std::env::current_dir()?.canonicalize()?
    };
    let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
        .context("Missing OS user")?;
    let _lock = if account.is_none() {
        Some(crate::InstanceLock::acquire(profile)?)
    } else {
        None
    };
    let account = match account {
        Some(a) => a,
        None => crate::account::Account::open(profile)?,
    };
    let owner = account
        .status()
        .user
        .map(|u| u.id)
        .context("Sign in to use the computer bridge")?;
    let key = crate::managed_access::proof::ProofKey::from_signing_key(signing_key(profile)?)?;
    let url = format!(
        "{}/v1/cloud/bridge/host-task",
        zuse_remote::normalized_origin(&crate::cloud::configured_api_origin())?
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let send = async |body: Value| -> Result<Value> {
        let credential = account.credential().await?;
        let proof = key.sign("POST", &url, Some(&credential.access_token))?;
        Ok(client
            .post(&url)
            .bearer_auth(credential.access_token)
            .header("dpop", proof)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    };
    let task = uuid::Uuid::new_v4();
    let preview = if dynamic {
        argv[0].clone()
    } else {
        serde_json::to_string(&argv)?
    };
    let journal = profile.join("computer-bridge").join("runs");
    std::fs::create_dir_all(&journal)?;
    for entry in std::fs::read_dir(&journal)? {
        let entry = entry?;
        let Some(request) = entry
            .file_name()
            .to_str()
            .and_then(|name| uuid::Uuid::parse_str(name).ok())
        else {
            continue;
        };
        if !entry.file_type()?.is_file() || entry.metadata()?.len() > 131072 {
            continue;
        }
        let bytes = Zeroizing::new(std::fs::read(entry.path())?);
        let Ok(saved) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if saved["state"] != "finished" {
            continue;
        }
        let result=send(json!({"action":"finish","task":saved["task"],"request":request,"status":saved["status"],"output":saved["output"]})).await;
        if let Err(error) = result {
            if error
                .downcast_ref::<reqwest::Error>()
                .and_then(|e| e.status())
                .map(|s| s.as_u16())
                != Some(409)
            {
                return Err(error);
            }
        }
    }
    loop {
        ensure!(
            account
                .status()
                .user
                .as_ref()
                .is_some_and(|u| u.id == owner),
            "The signed-in account changed"
        );
        send(json!({"action":"offer","task":task,"command":preview,"cwd":cwd,"user":user,"dynamic":dynamic})).await?;
        #[derive(serde::Deserialize)]
        struct Claimed {
            id: uuid::Uuid,
            command: String,
            cwd: std::path::PathBuf,
        }
        let claimed: Option<Claimed> =
            serde_json::from_value(send(json!({"action":"claim","task":task})).await?)?;
        if let Some(claimed) = claimed {
            let request = claimed.id;
            let mut run_argv = argv.clone();
            let run_cwd = if dynamic { claimed.cwd } else { cwd.clone() };
            if dynamic {
                #[cfg(windows)]
                run_argv.extend([
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    claimed.command,
                ]);
                #[cfg(not(windows))]
                run_argv.extend(["-c".into(), claimed.command]);
            }
            // A durable receipt is required before starting a process.
            let receipt = journal.join(request.to_string());
            let mut record=std::fs::OpenOptions::new().write(true).create_new(true).open(&receipt)
                .context("This command was already admitted. Check its outcome before requesting another run")?;
            record.write_all(
                serde_json::to_string(
                    &serde_json::json!({"task":task,"argv":run_argv,"cwd":run_cwd,"state":"admitted"}),
                )?
                .as_bytes(),
            )?;
            record.sync_all()?;
            #[cfg(unix)]
            std::fs::File::open(&journal)?.sync_all()?;
            let progress =
                std::sync::Arc::new(std::sync::Mutex::new((String::new(), String::new())));
            let revoked = async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let output = {
                        let current = progress.lock().unwrap_or_else(|e| e.into_inner());
                        format!("{}{}", current.0, current.1)
                    };
                    let allowed = send(
                        json!({"action":"check","task":task,"request":request,"output":output}),
                    )
                    .await;
                    if !matches!(allowed, Ok(Value::Bool(true))) {
                        return;
                    }
                }
            };
            let (status, output) = tokio::select! {
                done = run_task_progress(&run_argv,&run_cwd,Some(progress.clone())) => done,
                _ = revoked => ("authorization ended".to_owned(),String::new()),
                _ = tokio::signal::ctrl_c() => return Ok(()),
            };
            let mut saved = tempfile::NamedTempFile::new_in(&journal)?;
            saved.write_all(
                serde_json::to_string(
                    &json!({"task":task,"state":"finished","status":status,"output":output}),
                )?
                .as_bytes(),
            )?;
            saved.as_file().sync_all()?;
            saved.persist(&receipt)?;
            send(json!({"action":"finish","task":task,"request":request,"status":status,"output":output})).await?;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
async fn run_task(argv: &[String], cwd: &Path) -> (String, String) {
    run_task_progress(argv, cwd, None).await
}
type Progress = std::sync::Arc<std::sync::Mutex<(String, String)>>;
async fn run_task_progress(
    argv: &[String],
    cwd: &Path,
    progress: Option<Progress>,
) -> (String, String) {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    async fn drain(
        mut pipe: impl tokio::io::AsyncRead + Unpin,
        progress: Option<Progress>,
        stderr: bool,
    ) -> std::io::Result<String> {
        let mut kept = Vec::new();
        let mut buffer = [0; 4096];
        let mut truncated = false;
        loop {
            let n = pipe.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            let keep = n.min(12288 - kept.len());
            kept.extend_from_slice(&buffer[..keep]);
            truncated |= keep < n;
            if let Some(progress) = &progress {
                let mut current = progress.lock().unwrap_or_else(|e| e.into_inner());
                let text = String::from_utf8_lossy(&kept).into_owned();
                if stderr {
                    current.1 = text;
                } else {
                    current.0 = text;
                }
            }
        }
        let mut out = String::from_utf8_lossy(&kept).into_owned();
        if truncated {
            out.push_str("\n[output truncated]");
        }
        Ok(out)
    }
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ("start failed".into(), e.to_string()),
    };
    let _group = crate::project_creation::ProcessGroup(child.id());
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    match tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::try_join!(
            child.wait(),
            drain(stdout, progress.clone(), false),
            drain(stderr, progress, true)
        )
    })
    .await
    {
        Ok(Ok((status, out, err))) => (status.to_string(), format!("{out}{err}")),
        Ok(Err(e)) => ("output unavailable".into(), e.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            ("timed out".into(), String::new())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_argv_is_literal_and_preserves_nonzero_exit() {
        let argv = [
            "/bin/sh",
            "-c",
            "printf '%s' \"$1\"; exit 7",
            "check",
            "Unicode ✓ and $(literal)",
        ]
        .map(String::from);
        let (status, output) = run_task(&argv, Path::new("/tmp")).await;
        assert_eq!(output, "Unicode ✓ and $(literal)");
        assert_eq!(status, "exit status: 7");
    }

    #[tokio::test]
    async fn output_is_visible_before_process_exit() {
        let progress: Progress =
            std::sync::Arc::new(std::sync::Mutex::new((String::new(), String::new())));
        let reader = progress.clone();
        let job = tokio::spawn(async move {
            let argv = ["/bin/sh", "-c", "printf ready; sleep 30"].map(String::from);
            run_task_progress(&argv, Path::new("/tmp"), Some(progress)).await
        });
        let visible = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if reader.lock().unwrap().0 == "ready" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(!job.is_finished());
        job.abort();
        let _ = job.await;
        assert!(visible.is_ok());
    }

    #[tokio::test]
    async fn full_pipe_is_drained_after_output_limit() {
        let argv = ["/bin/sh", "-c", "head -c 65536 /dev/zero"].map(String::from);
        let (status, output) = run_task(&argv, Path::new("/tmp")).await;
        assert_eq!(status, "exit status: 0");
        assert_eq!(output.trim_end_matches("\n[output truncated]").len(), 12288);
        assert!(output.ends_with("[output truncated]"));
    }
}

#[derive(Default)]
pub(crate) struct Connector {
    task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for Connector {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().take() {
            task.abort();
        }
    }
}
impl Connector {
    pub async fn start(
        &self,
        profile: &Path,
        account: crate::account::Account,
        label: &str,
    ) -> Result<String> {
        let mut task = self.task.lock().await;
        let id = enroll_account(profile, label, &account).await?;
        if let Some(old) = task.take() {
            old.abort();
            let _ = old.await;
        }
        let profile = profile.to_owned();
        *task = Some(tokio::spawn(async move {
            let owner = account.status().user.map(|u| u.id);
            loop {
                if account.status().user.map(|u| u.id) != owner {
                    return;
                }
                let _ = serve_computer_account(&profile, Some(account.clone())).await;
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }));
        Ok(id)
    }
    pub async fn stop(&self) {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
    }
}
