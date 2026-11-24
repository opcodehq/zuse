//! Retained launch that recovers from an in-place runtime update the
//! computer's image can no longer launch.
//!
//! ```text
//! launch ── exit 0 ─────────────────────────────────────────────▶ launched
//!   └─ fails ─▶ repair (root, infra/cloud/runtime-update-launch-repair.py)
//!                ├─ not-applicable ─▶ provider failure (retried as before)
//!                ├─ failed ─────────▶ cloud_runtime_launch_incompatible
//!                └─ rolled-back ──▶ launch once more
//!                                     ├─ exit 0 ─▶ launched
//!                                     └─ fails ──▶ cloud_runtime_launch_incompatible
//! ```
//!
//! The image's harness gate refuses a runtime whose `--version` differs from
//! the version its harness release was built for. An in-place swap re-execs
//! without that gate, so such a computer works until its first cold launch,
//! which then fails on every retry (2026-11-24). The repair puts back the
//! binary the update replaced through the root helper's standard rollback.
use crate::providers::{ExecResult, ProviderError};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::machine_layout::Layout;

/// The runtime could not be launched even after its incompatible in-place
/// update was rolled back, or the rollback itself failed.
pub const LAUNCH_INCOMPATIBLE: &str = "cloud_runtime_launch_incompatible";

const REPAIR: &str = include_str!("../../../../infra/cloud/runtime-update-launch-repair.py");
/// Most launcher output kept for diagnosis (the tail, after scrubbing).
const TAIL_BYTES: usize = 1024;
/// Inside the lifecycle job's 120 s lease, with both launches.
const REPAIR_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs one shell command on a computer.
#[async_trait]
pub(crate) trait Exec: Send + Sync {
    async fn exec(&self, machine: &str, command: &str) -> Result<ExecResult, ProviderError>;
}

/// Why a retained launch failed, for the lifecycle's error code.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LaunchFailure {
    Provider(ProviderError),
    Incompatible,
}

#[derive(Deserialize)]
struct RepairResult {
    outcome: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default, rename = "fromVersion")]
    from_version: Option<String>,
    #[serde(default, rename = "toVersion")]
    to_version: Option<String>,
}

/// The repair command: the helper runs as root from stdin-free `-c`.
pub(crate) fn repair_command() -> String {
    format!(
        "sudo -n /usr/bin/python3 -I -B -c '{}'",
        REPAIR.replace('\'', "'\\''")
    )
}

/// Launch the retained runtime with `launch`; on failure, roll back an
/// incompatible in-place update once and launch again.
pub(crate) async fn launch(
    exec: &dyn Exec,
    machine: &str,
    workspace: &str,
    layout: Layout,
    launch: &str,
) -> Result<(), LaunchFailure> {
    let first = exec
        .exec(machine, launch)
        .await
        .map_err(LaunchFailure::Provider)?;
    if first.exit_code == 0 {
        return Ok(());
    }
    tracing::warn!(
        workspace,
        exit_code = first.exit_code,
        output = %tail(&first),
        "Retained runtime launch failed"
    );
    // Only root bases keep the root helper's journal and `previous` binary.
    if layout != Layout::Root {
        return Err(LaunchFailure::Provider(ProviderError::Unavailable));
    }
    let repaired =
        match tokio::time::timeout(REPAIR_TIMEOUT, exec.exec(machine, &repair_command())).await {
            Ok(Ok(result)) if result.exit_code == 0 => parse(&result.stdout),
            _ => None,
        };
    let Some(repaired) = repaired else {
        tracing::warn!(workspace, "Runtime launch repair gave no result");
        return Err(LaunchFailure::Provider(ProviderError::Unavailable));
    };
    match repaired.outcome.as_str() {
        "rolled-back" => {}
        "failed" => {
            tracing::warn!(
                workspace,
                code = repaired.code.as_deref().map(scrub_code),
                "Runtime launch repair failed"
            );
            return Err(LaunchFailure::Incompatible);
        }
        _ => {
            tracing::info!(
                workspace,
                code = repaired.code.as_deref().map(scrub_code),
                "Runtime launch repair not applicable"
            );
            return Err(LaunchFailure::Provider(ProviderError::Unavailable));
        }
    }
    tracing::warn!(
        workspace,
        from = repaired.from_version.as_deref().map(scrub_code),
        to = repaired.to_version.as_deref().map(scrub_code),
        "Rolled back an in-place runtime update the image could not launch"
    );
    let second = exec
        .exec(machine, launch)
        .await
        .map_err(LaunchFailure::Provider)?;
    if second.exit_code == 0 {
        return Ok(());
    }
    tracing::warn!(
        workspace,
        exit_code = second.exit_code,
        output = %tail(&second),
        "Retained runtime launch failed after the rollback"
    );
    Err(LaunchFailure::Incompatible)
}

fn parse(stdout: &[u8]) -> Option<RepairResult> {
    if stdout.len() > 4096 {
        return None;
    }
    let line = std::str::from_utf8(stdout)
        .ok()?
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())?;
    serde_json::from_str(line).ok()
}

/// A short machine-readable value from the guest, never free text.
fn scrub_code(value: &str) -> &str {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+:-".contains(&b))
    {
        value
    } else {
        "invalid"
    }
}

/// The last [`TAIL_BYTES`] of stderr (else stdout), printable ASCII only,
/// with anything that looks like a credential replaced.
pub(crate) fn tail(result: &ExecResult) -> String {
    let source = if result.stderr.is_empty() {
        &result.stdout
    } else {
        &result.stderr
    };
    let start = source.len().saturating_sub(TAIL_BYTES * 4);
    let text: String = String::from_utf8_lossy(&source[start..])
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let scrubbed = text
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(scrub_word)
        .collect::<Vec<_>>()
        .join(" ");
    let cut = scrubbed.len().saturating_sub(TAIL_BYTES);
    scrubbed[cut..].to_owned()
}

fn scrub_word(word: &str) -> String {
    const SECRET_KEYS: [&str; 6] = [
        "token",
        "secret",
        "key",
        "password",
        "credential",
        "authorization",
    ];
    if let Some((name, _)) = word.split_once('=') {
        let lower = name.to_ascii_lowercase();
        if SECRET_KEYS.iter().any(|k| lower.contains(k)) {
            return format!("{name}=[redacted]");
        }
    }
    // Long opaque runs (JWTs, API keys, signatures) are never kept.
    let opaque = word
        .split(|c: char| !(c.is_ascii_alphanumeric() || "-_+/.".contains(c)))
        .any(|run| {
            run.len() >= 32
                && run.chars().any(|c| c.is_ascii_digit())
                && run.chars().any(|c| c.is_ascii_alphabetic())
        });
    if opaque || word.to_ascii_lowercase().starts_with("bearer") {
        "[redacted]".to_owned()
    } else {
        word.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Script {
        answers: Mutex<Vec<Result<ExecResult, ProviderError>>>,
        commands: Mutex<Vec<String>>,
    }
    impl Script {
        fn new(answers: Vec<Result<ExecResult, ProviderError>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().rev().collect()),
                commands: Mutex::new(vec![]),
            }
        }
        fn commands(&self) -> Vec<String> {
            self.commands.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl Exec for Script {
        async fn exec(&self, _: &str, command: &str) -> Result<ExecResult, ProviderError> {
            self.commands.lock().unwrap().push(command.to_owned());
            self.answers.lock().unwrap().pop().expect("unexpected exec")
        }
    }
    fn exit(code: i32, stdout: &str, stderr: &str) -> Result<ExecResult, ProviderError> {
        Ok(ExecResult {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code: code,
        })
    }
    const LAUNCH: &str = "launch-rust-runtime";
    const ROLLED_BACK: &str = r#"{"fromSha256":"c","fromVersion":"0.26.1","outcome":"rolled-back","toSha256":"a","toVersion":"0.26.0","updateId":"u"}"#;

    #[tokio::test]
    async fn a_launch_that_works_runs_nothing_else() {
        let exec = Script::new(vec![exit(0, "", "")]);
        assert_eq!(launch(&exec, "m", "w", Layout::Root, LAUNCH).await, Ok(()));
        assert_eq!(exec.commands(), vec![LAUNCH]);
    }

    /// REGRESSION GUARD (2026-11-24): a computer updated in place to a version
    /// its image's harness gate refuses failed every resume until the deadline.
    #[tokio::test]
    async fn an_incompatible_update_is_rolled_back_and_launched_once_more() {
        let exec = Script::new(vec![
            exit(1, "", "harness release failed: ValueError"),
            exit(0, ROLLED_BACK, ""),
            exit(0, "", ""),
        ]);
        assert_eq!(launch(&exec, "m", "w", Layout::Root, LAUNCH).await, Ok(()));
        let commands = exec.commands();
        assert_eq!(commands.len(), 3);
        assert!(commands[1].starts_with("sudo -n /usr/bin/python3 -I -B -c '"));
        assert!(commands[1].contains("runtime_launch_repair_rollback_failed"));
        assert_eq!(commands[2], LAUNCH);
    }

    #[tokio::test]
    async fn a_failure_without_an_incompatible_update_stays_a_provider_failure() {
        let exec = Script::new(vec![
            exit(1, "", "runtime_supervisor_missing"),
            exit(
                0,
                r#"{"code":"runtime_update_not_committed","outcome":"not-applicable"}"#,
                "",
            ),
        ]);
        assert_eq!(
            launch(&exec, "m", "w", Layout::Root, LAUNCH).await,
            Err(LaunchFailure::Provider(ProviderError::Unavailable))
        );
        assert_eq!(exec.commands().len(), 2);
        // An unreadable or failed repair run is not evidence either.
        for answer in [
            exit(1, "", ""),
            exit(0, "not json", ""),
            Err(ProviderError::Unavailable),
        ] {
            let exec = Script::new(vec![exit(1, "", ""), answer]);
            assert_eq!(
                launch(&exec, "m", "w", Layout::Root, LAUNCH).await,
                Err(LaunchFailure::Provider(ProviderError::Unavailable))
            );
        }
        // Without the root helper layout nothing is attempted.
        let exec = Script::new(vec![exit(1, "", "")]);
        assert_eq!(
            launch(&exec, "m", "w", Layout::Home, LAUNCH).await,
            Err(LaunchFailure::Provider(ProviderError::Unavailable))
        );
        assert_eq!(exec.commands(), vec![LAUNCH]);
    }

    #[tokio::test]
    async fn a_failed_rollback_or_relaunch_is_reported_as_incompatible() {
        let exec = Script::new(vec![
            exit(1, "", ""),
            exit(
                0,
                r#"{"code":"runtime_launch_repair_rollback_failed","outcome":"failed"}"#,
                "",
            ),
        ]);
        assert_eq!(
            launch(&exec, "m", "w", Layout::Root, LAUNCH).await,
            Err(LaunchFailure::Incompatible)
        );
        let exec = Script::new(vec![
            exit(1, "", ""),
            exit(0, ROLLED_BACK, ""),
            exit(1, "", ""),
        ]);
        assert_eq!(
            launch(&exec, "m", "w", Layout::Root, LAUNCH).await,
            Err(LaunchFailure::Incompatible)
        );
        // Exec errors pass through unchanged.
        let exec = Script::new(vec![Err(ProviderError::NotFound)]);
        assert_eq!(
            launch(&exec, "m", "w", Layout::Root, LAUNCH).await,
            Err(LaunchFailure::Provider(ProviderError::NotFound))
        );
    }

    #[test]
    fn the_output_tail_is_bounded_and_scrubbed() {
        let secret = "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJ3b3Jrc3BhY2UifQ.c2lnbmF0dXJlMTIzNDU2Nzg5MA";
        let stderr = format!(
            "{}\nZUSE_RUNTIME_BOOT_TOKEN=abc password=hunter2 Bearer {secret} harness release failed: ValueError\x1b[0m",
            "x".repeat(5000)
        );
        let tail = tail(&ExecResult {
            stdout: b"ignored".to_vec(),
            stderr: stderr.into_bytes(),
            exit_code: 1,
        });
        assert!(tail.len() <= TAIL_BYTES);
        assert!(tail.ends_with("harness release failed: ValueError [0m"));
        assert!(tail.contains("ZUSE_RUNTIME_BOOT_TOKEN=[redacted]"));
        assert!(tail.contains("password=[redacted]"));
        for leaked in ["abc", "hunter2", secret, "\x1b"] {
            assert!(!tail.contains(leaked), "{leaked} leaked: {tail}");
        }
        // stdout stands in when stderr is empty.
        assert_eq!(
            super::tail(&ExecResult {
                stdout: b"runtime_supervisor_missing".to_vec(),
                stderr: vec![],
                exit_code: 1
            }),
            "runtime_supervisor_missing"
        );
    }

    #[test]
    fn the_repair_script_survives_shell_quoting_unchanged() {
        let command = repair_command();
        let printed = command.replacen("sudo -n /usr/bin/python3 -I -B -c", "printf %s", 1);
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &printed])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), REPAIR);
    }

    #[test]
    fn guest_values_are_logged_only_when_short_and_plain() {
        assert_eq!(scrub_code("0.26.1"), "0.26.1");
        assert_eq!(scrub_code("runtime_launchable"), "runtime_launchable");
        assert_eq!(scrub_code("a b"), "invalid");
        assert_eq!(scrub_code(&"a".repeat(65)), "invalid");
    }
}
