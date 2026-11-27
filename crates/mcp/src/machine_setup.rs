//! The image tools of a setup chat (see `zuse_protocol::machine_setup`).
//!
//! Reading and proposing go to the cloud through the engine, like the
//! computer tools. Testing a script is the one tool that runs something
//! here: this server lives on the setup computer, which is exactly where the
//! script should be tried before it is built into an image.

use std::time::Duration;

use anyhow::Context as _;
use serde_json::{Value, json};
use zuse_protocol::machine_setup::{DEFAULT_TEST_TIMEOUT_SECS, MAX_TEST_TIMEOUT_SECS};

use crate::tools::ToolDef;

pub const GET: &str = "get_machine_image_config";
pub const PROPOSE: &str = "propose_machine_image_config";
pub const TEST: &str = "test_setup_script";
pub const NAMES: [&str; 3] = [GET, PROPOSE, TEST];

/// Bytes of script output a test returns: the end, where failures are.
const OUTPUT_TAIL: usize = 12 * 1024;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: GET,
            description: "Read this account's machine image configuration: the setup script every image build runs, the repositories baked into the image (github.com/owner/name, with whether the current image carries them) and the image state. Call it before proposing changes.",
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        },
        ToolDef {
            name: PROPOSE,
            description: "Propose a new machine image configuration for the user to review. Pass the COMPLETE setup script (it replaces the current one) and the COMPLETE list of repositories the image should carry. The user sees the change in this chat with Apply & rebuild and Discard; applying saves it and rebuilds the image, and the current image keeps serving until the new one is verified. Test the script with test_setup_script first. Never include secrets. Returns the proposal id; don't describe the card.",
            input_schema: json!({"type":"object","additionalProperties":false,"required":["setup_script","repositories"],"properties":{
                "setup_script":{"type":"string","maxLength":65536,"description":"The full bash setup script: idempotent and non-interactive."},
                "repositories":{"type":"array","maxItems":zuse_protocol::machine_setup::MAX_REPOSITORIES,"items":{"type":"string"},"description":"Every repository the image should carry, as github.com/owner/name. Omitting a current one removes it."},
                "notes":{"type":"string","maxLength":zuse_protocol::machine_setup::MAX_NOTES,"description":"One or two sentences for the user: what changes and why."}
            }}),
        },
        ToolDef {
            name: TEST,
            description: "Run a setup script on this setup computer with bash -euo pipefail, as the image build would, and wait for it. Returns the exit code, whether it timed out, and the end of its combined output (secrets redacted). Changes it makes stay on this computer only; the image changes only when the user applies a proposal.",
            input_schema: json!({"type":"object","additionalProperties":false,"required":["setup_script"],"properties":{
                "setup_script":{"type":"string","maxLength":65536},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":MAX_TEST_TIMEOUT_SECS,"default":DEFAULT_TEST_TIMEOUT_SECS}
            }}),
        },
    ]
}

/// The card a proposal shows in the calling chat.
pub fn proposal_spec(id: &str) -> String {
    let quote = serde_json::to_string(id).expect("strings serialize");
    format!("root = MachineImageProposal({quote})")
}

/// Plain words for a refused proposal, so the agent can fix it.
pub fn explain(error: anyhow::Error) -> anyhow::Error {
    let text = format!("{error:#}");
    let hint = [
        (
            "machine_setup_script_contains_secret",
            "The script contains what looks like a credential. Remove it: images must never carry secrets.",
        ),
        (
            "machine_setup_repository_invalid",
            "A repository is not in the form github.com/owner/name.",
        ),
        (
            "machine_setup_too_many_repositories",
            "Too many repositories: an image carries at most 50.",
        ),
        (
            "machine_setup_notes_too_long",
            "Keep the notes to a few sentences.",
        ),
        (
            "machine_recipe_text_too_large",
            "The setup script is over 64 KiB. Shorten it.",
        ),
        (
            "machine_setup_chat_required",
            "This chat is not a setup chat; image tools only work in one started from Settings.",
        ),
    ]
    .into_iter()
    .find(|(code, _)| text.contains(code));
    match hint {
        Some((_, hint)) => anyhow::anyhow!(hint),
        None => error,
    }
}

/// Lines that look like they carry a credential are replaced.
fn redact(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            if zuse_protocol::machine_setup::secret_in_script(line) {
                "[redacted]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tail(text: &str) -> (String, bool) {
    if text.len() <= OUTPUT_TAIL {
        return (text.to_owned(), false);
    }
    let mut start = text.len() - OUTPUT_TAIL;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_owned(), true)
}

/// Stops a test run's process group and removes its script when dropped.
struct Cleanup {
    path: std::path::PathBuf,
    group: Option<u32>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group {
            // Harmless when the group already exited.
            if let Ok(group) = libc::pid_t::try_from(group) {
                // SAFETY: kill(2) with a negative pid signals that process
                // group; it touches no memory of ours.
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Run `script` here and report how it went.
pub async fn test_script(script: &str, timeout_seconds: Option<u64>) -> anyhow::Result<Value> {
    anyhow::ensure!(!script.trim().is_empty(), "setup_script is empty");
    anyhow::ensure!(
        script.len() <= 64 * 1024 && !script.contains('\0'),
        "setup_script must be text under 64 KiB"
    );
    let seconds = timeout_seconds.unwrap_or(u64::from(DEFAULT_TEST_TIMEOUT_SECS));
    anyhow::ensure!(
        (1..=u64::from(MAX_TEST_TIMEOUT_SECS)).contains(&seconds),
        "timeout_seconds must be between 1 and {MAX_TEST_TIMEOUT_SECS}"
    );
    let path = std::env::temp_dir().join(format!(
        "zuse-setup-test-{}.sh",
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::write(&path, script).await?;
    let started = std::time::Instant::now();
    let mut command = tokio::process::Command::new("bash");
    // One stream, in order, like the build log.
    command
        .arg("-c")
        .arg("exec 2>&1; exec bash -euo pipefail \"$0\"")
        .arg(&path)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    // From here on, a cancelled call (the agent stopped) or a deadline
    // still stops the script's whole process group and removes the file.
    let mut cleanup = Cleanup {
        path: path.clone(),
        group: None,
    };
    let mut child = command.spawn()?;
    cleanup.group = child.id();
    let mut stdout = child.stdout.take().context("script output unavailable")?;
    // Only the end is kept, so a chatty script can't fill memory.
    let mut kept: Vec<u8> = Vec::new();
    let mut dropped = false;
    let run = async {
        use tokio::io::AsyncReadExt;
        let mut chunk = [0u8; 8192];
        loop {
            let n = stdout.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&chunk[..n]);
            if kept.len() > 2 * OUTPUT_TAIL {
                kept.drain(..kept.len() - OUTPUT_TAIL);
                dropped = true;
            }
        }
        child.wait().await
    };
    let outcome = tokio::time::timeout(Duration::from_secs(seconds), run).await;
    let (exit_code, timed_out) = match outcome {
        Ok(status) => (status?.code(), false),
        Err(_) => (None, true),
    };
    drop(cleanup);
    let mut output = String::from_utf8_lossy(&kept).into_owned();
    if dropped {
        // The first kept line may be cut; start at a whole line.
        if let Some(at) = output.find('\n') {
            output.drain(..=at);
        }
    }
    let (output, cut) = tail(&redact(&output));
    let truncated = cut || dropped;
    Ok(json!({
        "exitCode": exit_code,
        "succeeded": exit_code == Some(0),
        "timedOut": timed_out,
        "seconds": started.elapsed().as_secs(),
        "output": output,
        "outputTruncated": truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_test_run_reports_exit_code_and_redacted_output_in_order() {
        let ok = test_script("echo one\necho two >&2\n", Some(30))
            .await
            .unwrap();
        assert_eq!(ok["succeeded"], true);
        assert_eq!(ok["output"], "one\ntwo");
        let failed = test_script(
            "echo NPM_TOKEN=abcdefghijklmnopqrstuvwxyz\nfalse\necho never\n",
            Some(30),
        )
        .await
        .unwrap();
        assert_eq!(failed["exitCode"], 1);
        assert_eq!(failed["output"], "[redacted]", "pipefail stops at false");
        let unset = test_script("echo \"$NOT_SET_ANYWHERE\"", Some(30))
            .await
            .unwrap();
        assert_eq!(unset["succeeded"], false, "-u catches unset variables");
    }

    #[tokio::test]
    async fn a_test_run_stops_at_its_deadline() {
        let slow = test_script("echo installing\nsleep 30", Some(1))
            .await
            .unwrap();
        assert_eq!(slow["timedOut"], true);
        assert_eq!(
            slow["output"], "installing",
            "what it printed before the deadline"
        );
        assert!(slow["seconds"].as_u64().unwrap() < 10);
        assert!(test_script("true", Some(0)).await.is_err());
        assert!(test_script("  ", None).await.is_err());
    }

    #[tokio::test]
    async fn a_chatty_script_keeps_only_its_end() {
        let chatty = test_script("seq 1 1500000 | sed s/.*/x/\necho done", Some(60))
            .await
            .unwrap();
        assert_eq!(chatty["succeeded"], true);
        assert_eq!(chatty["outputTruncated"], true);
        let output = chatty["output"].as_str().unwrap();
        assert!(output.len() <= OUTPUT_TAIL && output.ends_with("x\ndone"));
    }

    #[tokio::test]
    async fn a_cancelled_run_stops_the_scripts_children() {
        let marker =
            std::env::temp_dir().join(format!("zuse-late-{}", uuid::Uuid::new_v4().simple()));
        let script = format!("(sleep 2; touch {}) &\nsleep 30", marker.display());
        let run = tokio::spawn(async move { test_script(&script, Some(60)).await });
        tokio::time::sleep(Duration::from_millis(500)).await;
        run.abort();
        let _ = run.await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!marker.exists(), "the background child was stopped");
    }

    #[test]
    fn output_keeps_the_end() {
        let long = "x".repeat(OUTPUT_TAIL + 10) + "end";
        let (kept, truncated) = tail(&long);
        assert!(truncated);
        assert!(kept.ends_with("end"));
        assert_eq!(kept.len(), OUTPUT_TAIL);
        assert_eq!(
            proposal_spec("msp_1"),
            "root = MachineImageProposal(\"msp_1\")"
        );
    }
}
