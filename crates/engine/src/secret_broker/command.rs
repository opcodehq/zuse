//! Explicitly weaker than HTTPS: the approved process can read and retain the
//! credential. Withholding output prevents accidental tool-result disclosure;
//! it does not isolate the credential from the process or the agent OS identity.
use super::{Scope, approve};
use serde::Deserialize;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;
use zuse_harness::{Error, ToolContext};
use zuse_protocol::UserInputQuestion;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    mode: String,
    label: String,
    reason: String,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    delivery: Delivery,
    env_var: Option<String>,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Delivery {
    Stdin,
    Env,
}
fn default_timeout() -> u64 {
    60
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.contains('\0')
}
impl Operation {
    fn parse(input: serde_json::Value) -> Result<Self, Error> {
        let invalid = || Error::Tool("Invalid private command operation".into());
        let op: Self = serde_json::from_value(input).map_err(|_| invalid())?;
        if op.mode != "command"
            || !valid_text(&op.label, 200)
            || !valid_text(&op.reason, 2000)
            || !valid_text(&op.program, 2048)
            || !std::path::Path::new(&op.program).is_absolute()
            || !op.cwd.is_absolute()
            || !valid_text(&op.cwd.to_string_lossy(), 4096)
            || op.args.len() > 128
            || op.args.iter().any(|a| a.contains('\0'))
            || op.args.iter().map(|a| a.len()).sum::<usize>() > 16000
            || !(1..=300).contains(&op.timeout_seconds)
        {
            return Err(invalid());
        }
        match (&op.delivery, &op.env_var) {
            (Delivery::Stdin, None) => {}
            (Delivery::Env, Some(name))
                if name.len() <= 128
                    && name.bytes().enumerate().all(|(i, c)| {
                        c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                    })
                    && !name.is_empty() => {}
            _ => return Err(invalid()),
        }
        Ok(op)
    }
    fn question(&self) -> UserInputQuestion {
        let delivery = match &self.delivery {
            Delivery::Stdin => "stdin (exact bytes, then EOF)".to_owned(),
            Delivery::Env => format!("environment variable {}", self.env_var.as_deref().unwrap()),
        };
        UserInputQuestion {
            id: String::new(),
            header: "Private command".into(),
            question: format!(
                "{}\n{}\n\nApprove one command on the execution host:\nProgram: {}\nArguments (JSON): {}\nWorking directory: {}\nSecret delivery: {}\nTimeout: {} seconds\n\nThe command can read, save, or transmit this secret. Files it writes may be readable by the agent later. Only completion status is returned; stdout and stderr are discarded. Expires in 10 minutes.",
                self.label,
                self.reason,
                serde_json::to_string(&self.program).unwrap(),
                serde_json::to_string(&self.args).unwrap(),
                serde_json::to_string(&self.cwd).unwrap(),
                delivery,
                self.timeout_seconds
            ),
            approval_prompt: None,
            approval: None,
            options: vec![],
            multi_select: false,
            prefill: None,
            multiline: false,
        }
    }
}

// Kill the process group on completion, cancellation, timeout, or a dropped future.
// kill_on_drop additionally owns the immediate child on all platforms.
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            unsafe {
                libc::kill(-(id as i32), libc::SIGKILL);
            }
        }
    }
}

pub(super) async fn execute<F, Fut>(
    input: serde_json::Value,
    context: ToolContext,
    ask: F,
) -> Result<String, Error>
where
    F: FnOnce(UserInputQuestion) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>, Error>>,
{
    if context.read_only {
        return Err(Error::Tool(
            "Private commands are unavailable in plan mode".into(),
        ));
    }
    let op = Operation::parse(input)?;
    let question = op.question();
    let scope = Scope {
        project: uuid::Uuid::new_v4().to_string(),
        session: context.agent_id.clone(),
        run: context.call_id.clone(),
        operation: question.question.clone(),
    };
    let Some(token) = approve(scope, context.clone(), question, ask).await? else {
        return Ok("Secret request declined".into());
    };
    run(&op, token, &context).await
}

async fn run(
    op: &Operation,
    token: Zeroizing<String>,
    context: &ToolContext,
) -> Result<String, Error> {
    let fail = || Error::Tool("Private command failed; output withheld".into());
    let mut command = tokio::process::Command::new(&op.program);
    command
        .args(&op.args)
        .current_dir(&op.cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match &op.delivery {
        Delivery::Stdin => {
            command.stdin(Stdio::piped());
        }
        Delivery::Env => {
            if token.contains('\0') {
                return Err(fail());
            }
            command
                .stdin(Stdio::null())
                .env(op.env_var.as_ref().unwrap(), token.as_str());
        }
    }
    #[cfg(unix)]
    {
        command.process_group(0);
        // Set the mask only in the child, without changing the engine or other
        // concurrent commands. Programs may still explicitly override file modes.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                Ok(())
            });
        }
    }
    let work = async {
        if context.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        let mut child = command.spawn().map_err(|_| fail())?;
        let _group = Group(child.id());
        // Drop the command builder's environment copy before waiting.
        drop(command);
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(token.as_bytes())
                .await
                .map_err(|_| fail())?;
            drop(stdin);
        }
        drop(token);
        let status = child.wait().await.map_err(|_| fail())?;
        // Do not return exit codes: a command may encode credential bytes in them.
        Ok(if status.success() {
            "Private command completed successfully. Output withheld."
        } else {
            "Private command failed. Output withheld."
        }
        .into())
    };
    tokio::select! {
        biased;
        _ = context.cancel.cancelled() => Err(Error::Interrupted),
        result = tokio::time::timeout(Duration::from_secs(op.timeout_seconds), work) =>
            result.map_err(|_| Error::Tool("Private command timed out; output withheld".into()))?,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::secret_broker::{execute, shared};
    use serde_json::json;

    const SENTINEL: &str = "synthetic-private-command-sentinel";
    fn context() -> ToolContext {
        ToolContext {
            agent_id: "command-test".into(),
            call_id: uuid::Uuid::new_v4().to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            permission_mode: zuse_harness::PermissionMode::Default,
            read_only: false,
        }
    }
    fn input(dir: &std::path::Path, script: &str) -> serde_json::Value {
        json!({"mode":"command", "label":"CLI key", "reason":"Configure test CLI",
            "program":"/bin/sh", "args":["-c", script], "cwd":dir,
            "delivery":"stdin", "timeout_seconds":2})
    }
    fn submit(q: UserInputQuestion) -> std::future::Ready<Result<Vec<String>, Error>> {
        assert!(!q.question.contains(SENTINEL));
        assert!(q.question.contains("can read, save, or transmit"));
        let id = q.id.strip_prefix("private-secret:").unwrap();
        let mut broker = shared();
        let scope = broker.entries[id].scope.clone();
        broker
            .submit(id, &scope, Zeroizing::new(SENTINEL.into()))
            .unwrap();
        std::future::ready(Ok(vec!["saved".into()]))
    }
    #[tokio::test]
    async fn stdin_and_environment_deliver_without_exposing_output() {
        let dir = tempfile::tempdir().unwrap();
        // Test intentional configuration persistence as well as malicious echoes
        // on both output streams. Nothing is captured into a tool result.
        let result = execute(
            input(dir.path(), "cat > config; cat config; cat config >&2"),
            context(),
            submit,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config")).unwrap(),
            SENTINEL
        );
        assert_eq!(
            result,
            "Private command completed successfully. Output withheld."
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("config"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let mut i = input(
            dir.path(),
            "printf '%s' \"$CLI_TOKEN\" > config; printf '%s' \"$CLI_TOKEN\"; printf '%s' \"$CLI_TOKEN\" >&2",
        );
        i["delivery"] = json!("env");
        i["env_var"] = json!("CLI_TOKEN");
        let result = execute(i, context(), submit).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config")).unwrap(),
            SENTINEL
        );
        assert!(!result.contains(SENTINEL));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[tokio::test]
    async fn invalid_declined_and_read_only_never_execute() {
        let dir = tempfile::tempdir().unwrap();
        let base = input(dir.path(), "touch executed");
        for (key, value) in [
            ("program", json!("sh")),
            ("cwd", json!("relative")),
            ("delivery", json!("argument")),
            ("timeout_seconds", json!(301)),
            ("env_var", json!("NOT VALID")),
            ("mode", json!("other")),
            ("url", json!("https://example.com")),
            ("args", json!(["bad\u{0000}arg"])),
        ] {
            let mut i = base.clone();
            i[key] = value;
            assert!(
                execute(i, context(), |_| async {
                    panic!("invalid operation prompted")
                })
                .await
                .is_err()
            );
        }
        let mut ctx = context();
        ctx.read_only = true;
        assert!(
            execute(base.clone(), ctx, |_| async {
                panic!("read-only prompted")
            })
            .await
            .is_err()
        );
        assert_eq!(
            execute(base, context(), |_| async { Ok(vec!["declined".into()]) })
                .await
                .unwrap(),
            "Secret request declined"
        );
        assert!(!dir.path().join("executed").exists());
    }
    #[tokio::test]
    async fn failures_and_timeouts_withhold_values() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute(input(dir.path(), "cat >&2; exit 42"), context(), submit)
            .await
            .unwrap();
        assert_eq!(result, "Private command failed. Output withheld.");
        let mut missing = input(dir.path(), "");
        missing["program"] = json!("/does/not/exist");
        assert_eq!(
            execute(missing, context(), submit)
                .await
                .unwrap_err()
                .to_string(),
            "Private command failed; output withheld"
        );
        let mut slow = input(dir.path(), "sleep 30; touch escaped");
        slow["timeout_seconds"] = json!(1);
        let error = execute(slow, context(), submit)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("timed out"));
        assert!(!error.contains(SENTINEL));
        assert!(!dir.path().join("escaped").exists());
    }
    #[tokio::test]
    async fn cancellation_stops_a_running_command() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context();
        let cancel = ctx.cancel.clone();
        let cancel_when_started = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !dir.path().join("started").exists() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            cancel.cancel();
        };
        let (result, _) = tokio::join!(
            execute(
                input(dir.path(), "touch started; sleep 30; touch escaped"),
                ctx,
                submit
            ),
            cancel_when_started
        );
        assert!(matches!(result, Err(Error::Interrupted)));
        assert!(!dir.path().join("escaped").exists());
    }

    #[tokio::test]
    async fn cancellation_and_dropped_approval_revoke_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context();
        let cancel = ctx.cancel.clone();
        let mut captured = String::new();
        let result = execute(input(dir.path(), "touch executed"), ctx, |q| {
            captured = q.id.strip_prefix("private-secret:").unwrap().into();
            let reply = submit(q);
            cancel.cancel();
            reply
        })
        .await;
        assert!(matches!(result, Err(Error::Interrupted)));
        assert!(!shared().entries.contains_key(&captured));
        assert!(!dir.path().join("executed").exists());
        let mut captured = String::new();
        {
            let future = execute(input(dir.path(), "touch executed"), context(), |q| {
                captured = q.id.strip_prefix("private-secret:").unwrap().into();
                drop(submit(q));
                std::future::pending::<Result<Vec<String>, Error>>()
            });
            tokio::pin!(future);
            tokio::select! {
                _ = &mut future => panic!("must wait for approval"),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {},
            }
        }
        assert!(!shared().entries.contains_key(&captured));
    }
}
