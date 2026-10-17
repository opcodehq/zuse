#![cfg(unix)]
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use zuse_harness::{Error, PermissionMode, ToolContext, processes::*};
#[derive(Default)]
struct Notices {
    values: Mutex<Vec<ProcessNotice>>,
    fail: AtomicBool,
}
#[async_trait]
impl ProcessNotifier for Notices {
    async fn notify(&self, notice: ProcessNotice) -> Result<(), Error> {
        if self.fail.load(Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let mut values = self.values.lock().unwrap();
        if !values.iter().any(|prior| prior.id == notice.id) {
            values.push(notice);
        }
        Ok(())
    }
}
fn context(agent: &str) -> ToolContext {
    ToolContext {
        agent_id: agent.into(),
        call_id: "fixture-call".into(),
        cancel: CancellationToken::new(),
        permission_mode: PermissionMode::Default,
        read_only: false,
    }
}
fn command(text: &str, cwd: PathBuf) -> ProcessStart {
    ProcessStart {
        file: "/bin/bash".into(),
        args: vec!["-c".into(), text.into()],
        cwd,
        label: "fixture command".into(),
        timeout_ms: 10000,
        notify_on_output: None,
        notify: true,
    }
}
fn options(root: &std::path::Path, notices: Arc<Notices>) -> ProcessOptions {
    let mut options = ProcessOptions::new("root".into(), root.join("artifacts"), notices).unwrap();
    options.command_host = PathBuf::from(env!("CARGO_BIN_EXE_zuse-command-host"));
    options
}
async fn completed(manager: &Processes, id: &str) -> ProcessRecord {
    tokio::time::timeout(
        Duration::from_secs(6),
        manager.wait(id, "root", 30000, &CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap()
}
#[tokio::test]
async fn owned_commands_stream_stdin_preserve_byte_cursors_and_deliver_stable_notices() {
    let root = tempfile::tempdir().unwrap();
    let notices = Arc::new(Notices::default());
    let manager = Processes::open(options(root.path(), notices.clone()))
        .await
        .unwrap();
    let owner = context("child-a");
    let mut start = command(
        "printf 'ready\\n'; read line; printf '%s 世界\\n' \"$line\"; printf 'stderr\\n' >&2",
        root.path().into(),
    );
    start.notify_on_output = Some("ready".into());
    let process = manager.start(start, &owner).await.unwrap();
    assert!(
        manager
            .output(&process.id, "child-b", 0, 100)
            .await
            .is_err()
    );
    assert!(manager.stop(&process.id, "child-b").await.is_err());
    manager
        .write(&process.id, "child-a", "input\n", true, &owner.cancel)
        .await
        .unwrap();
    let record = completed(&manager, &process.id).await;
    assert_eq!(record.status, ProcessStatus::Exited);
    assert_eq!(record.exit_code, Some(0));
    let mut cursor = 0;
    let mut text = String::new();
    loop {
        let page = manager
            .output(&process.id, "root", cursor, 4)
            .await
            .unwrap();
        if page.next_cursor == cursor {
            break;
        }
        cursor = page.next_cursor;
        text.push_str(&page.text);
    }
    assert!(text.contains("input 世界"));
    assert!(text.contains("stderr"));
    assert!(!text.contains('\u{fffd}'));
    assert!(
        manager
            .output(&process.id, "root", cursor + 1, 100)
            .await
            .is_err()
    );
    let values = notices.values.lock().unwrap().clone();
    assert_eq!(values.iter().filter(|n| n.kind == "completed").count(), 1);
    assert_eq!(values.iter().filter(|n| n.kind == "ready").count(), 1);
    assert!(
        values
            .iter()
            .all(|n| n.agent_id == "child-a" && n.id.starts_with(&process.id))
    );
    manager.close().await;
}
#[tokio::test]
async fn output_limits_deadlines_and_agent_interrupts_reap_commands() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = options(root.path(), Arc::new(Notices::default()));
    settings.max_output_bytes = 128;
    let manager = Processes::open(settings).await.unwrap();
    let owner = context("child-a");
    let loud = manager
        .start(
            command(
                "while :; do printf 'abcdefghijklmno'; done",
                root.path().into(),
            ),
            &owner,
        )
        .await
        .unwrap();
    assert_eq!(
        completed(&manager, &loud.id).await.status,
        ProcessStatus::OutputLimit
    );
    let output = manager.output(&loud.id, "root", 0, 64000).await.unwrap();
    assert_eq!(output.next_cursor, 128);
    assert_eq!(std::fs::metadata(output.artifact).unwrap().len(), 128);
    let mut short = command("sleep 30", root.path().into());
    short.timeout_ms = 100;
    let short = manager.start(short, &owner).await.unwrap();
    assert_eq!(
        completed(&manager, &short.id).await.status,
        ProcessStatus::TimedOut
    );
    let waiting = manager
        .start(command("sleep 30", root.path().into()), &owner)
        .await
        .unwrap();
    manager.interrupt_agents(&["child-a".into()]).await;
    assert_eq!(
        manager
            .output(&waiting.id, "root", 0, 100)
            .await
            .unwrap()
            .process
            .status,
        ProcessStatus::Cancelled
    );
    manager.close().await;
    assert!(
        manager
            .start(command("touch must-not-exist", root.path().into()), &owner)
            .await
            .is_err()
    );
    assert!(!root.path().join("must-not-exist").exists());
}
#[tokio::test]
async fn restart_recovers_the_notification_outbox_without_replaying_commands() {
    let root = tempfile::tempdir().unwrap();
    let notices = Arc::new(Notices::default());
    notices.fail.store(true, Ordering::Release);
    let settings = options(root.path(), notices.clone());
    let manager = Processes::open(settings).await.unwrap();
    let owner = context("root");
    let process = manager
        .start(
            command(
                "printf 'once' >> execution-count; printf 'done'",
                root.path().into(),
            ),
            &owner,
        )
        .await
        .unwrap();
    assert_eq!(
        completed(&manager, &process.id).await.status,
        ProcessStatus::Exited
    );
    manager.close().await;
    drop(manager);
    assert!(notices.values.lock().unwrap().is_empty());
    notices.fail.store(false, Ordering::Release);
    let manager = Processes::open(options(root.path(), notices.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !notices.values.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("execution-count")).unwrap(),
        "once"
    );
    assert_eq!(notices.values.lock().unwrap().len(), 1);
    manager.close().await;
    drop(manager);
    let manager = Processes::open(options(root.path(), notices.clone()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(notices.values.lock().unwrap().len(), 1);
    manager.close().await;
}
#[tokio::test]
async fn dropping_the_manager_closes_its_private_control_pipe_and_stops_descendants() {
    let root = tempfile::tempdir().unwrap();
    let manager = Processes::open(options(root.path(), Arc::new(Notices::default())))
        .await
        .unwrap();
    let owner = context("root");
    let process = manager
        .start(
            command(
                "sleep 30 & echo $! > descendant.pid; wait",
                root.path().into(),
            ),
            &owner,
        )
        .await
        .unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(root.path().join("descendant.pid")).await {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(manager);
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            // A killed child can briefly remain a zombie until init reaps it.
            #[cfg(target_os = "linux")]
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
                s.split_once(") ")
                    .is_some_and(|(_, tail)| tail.starts_with('Z'))
            });
            #[cfg(not(target_os = "linux"))]
            let zombie = false;
            if !alive || zombie {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let settings = options(root.path(), Arc::new(Notices::default()));
    let recovered = Processes::open(settings).await.unwrap();
    let status = recovered
        .output(&process.id, "root", 0, 100)
        .await
        .unwrap()
        .process
        .status;
    assert!(matches!(
        status,
        ProcessStatus::Cancelled | ProcessStatus::Interrupted
    ));
    recovered.close().await;
}
#[tokio::test]
async fn shutdown_racing_command_admission_waits_for_every_admitted_worker() {
    let root = tempfile::tempdir().unwrap();
    let manager = Processes::open(options(root.path(), Arc::new(Notices::default())))
        .await
        .unwrap();
    let mut tasks = vec![];
    for _ in 0..8 {
        let manager = manager.clone();
        let cwd = root.path().to_path_buf();
        tasks.push(tokio::spawn(async move {
            manager
                .start(command("sleep 30", cwd), &context("root"))
                .await
        }));
    }
    tokio::task::yield_now().await;
    manager.close().await;
    for task in tasks {
        let _ = task.await.unwrap();
    }
    assert!(
        manager
            .list("root")
            .iter()
            .all(|p| p.status != ProcessStatus::Running)
    );
}

#[tokio::test]
async fn supervisor_requires_launch_ack_and_control_eof_releases_its_command_lease() {
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::{FramedRead, LinesCodec};
    let root = tempfile::tempdir().unwrap();
    let lease = root.path().join("command.lease");
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_zuse-command-host"));
    process
        .arg(supervisor::FLAG)
        .process_group(0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut process = process.spawn().unwrap();
    let mut control = process.stdin.take().unwrap();
    let mut output = FramedRead::new(
        process.stdout.take().unwrap(),
        LinesCodec::new_with_max_length(supervisor::MAX_FRAME),
    );
    let spec = serde_json::json!({"file":"/bin/bash","args":["-c","sleep 30 & echo $! > descendant.pid; wait"],"cwd":root.path(),"env":null,"timeout_ms":10000,"lease":lease});
    control
        .write_all(format!("{spec}\n").as_bytes())
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), output.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&frame).unwrap()["kind"],
        "ready"
    );
    assert!(!root.path().join("descendant.pid").exists());
    let probe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lease)
        .unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    control.write_all(b"{\"kind\":\"launch\"}\n").await.unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), output.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&frame).unwrap()["kind"],
        "started"
    );
    let descendant = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(value) = tokio::fs::read_to_string(root.path().join("descendant.pid")).await {
                if let Ok(pid) = value.trim().parse::<i32>() {
                    break pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(control);
    tokio::time::timeout(Duration::from_secs(3), process.wait())
        .await
        .unwrap()
        .unwrap();
    probe.try_lock().unwrap();
    probe.unlock().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let alive = unsafe { libc::kill(descendant, 0) } == 0;
            #[cfg(target_os = "linux")]
            let zombie =
                std::fs::read_to_string(format!("/proc/{descendant}/stat")).is_ok_and(|s| {
                    s.split_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with('Z'))
                });
            #[cfg(not(target_os = "linux"))]
            let zombie = false;
            if !alive || zombie {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn journal_recovery_waits_for_the_old_commands_physical_lease() {
    let root = tempfile::tempdir().unwrap();
    let notices = Arc::new(Notices::default());
    let manager = Processes::open(options(root.path(), notices.clone()))
        .await
        .unwrap();
    let process = manager
        .start(
            command("echo running > started; sleep 30", root.path().into()),
            &context("root"),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !root.path().join("started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let settings = options(root.path(), notices.clone());
    let recovery = tokio::spawn(async move { Processes::open(settings).await });
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(!recovery.is_finished());
    manager.stop(&process.id, "root").await.unwrap();
    let recovered = tokio::time::timeout(Duration::from_secs(3), recovery)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_ne!(
        recovered
            .output(&process.id, "root", 0, 100)
            .await
            .unwrap()
            .process
            .status,
        ProcessStatus::Running
    );
    manager.close().await;
    recovered.close().await;
}

#[tokio::test]
async fn exited_commands_keep_complete_output_frames_when_the_reader_is_slow() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use futures::StreamExt;
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::{FramedRead, LinesCodec};
    let root = tempfile::tempdir().unwrap();
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_zuse-command-host"));
    process
        .arg(supervisor::FLAG)
        .process_group(0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut process = process.spawn().unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = FramedRead::new(
        process.stdout.take().unwrap(),
        LinesCodec::new_with_max_length(supervisor::MAX_FRAME),
    );
    let spec = serde_json::json!({"file":"/bin/bash","args":["-c","head -c 131072 /dev/zero"],"cwd":root.path(),"env":null,"timeout_ms":10000,"lease":root.path().join("command.lease")});
    input
        .write_all(format!("{spec}\n").as_bytes())
        .await
        .unwrap();
    let frame = output.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&frame).unwrap()["kind"],
        "ready"
    );
    input.write_all(b"{\"kind\":\"launch\"}\n").await.unwrap();
    let frame = output.next().await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&frame).unwrap()["kind"],
        "started"
    );
    // The command can exit while its supervisor is blocked on the reader.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let mut received = 0;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame: serde_json::Value =
                serde_json::from_str(&output.next().await.unwrap().unwrap()).unwrap();
            if frame["kind"] == "exit" {
                assert_eq!(frame["code"], 0);
                break;
            }
            assert_eq!(frame["kind"], "output");
            let bytes = STANDARD.decode(frame["data"].as_str().unwrap()).unwrap();
            assert!(bytes.iter().all(|b| *b == 0));
            received += bytes.len();
        }
    })
    .await
    .unwrap();
    assert_eq!(received, 131072);
    process.wait().await.unwrap();
}

#[tokio::test]
async fn only_returned_commands_outlive_turn_cancellation_and_explicit_stop_still_owns_them() {
    let root = tempfile::tempdir().unwrap();
    let manager = Processes::open(options(root.path(), Arc::new(Notices::default())))
        .await
        .unwrap();
    let owner = context("child-a");
    let waiting = manager
        .start(
            command("read line; printf '%s' \"$line\"", root.path().into()),
            &owner,
        )
        .await
        .unwrap();
    assert!(
        manager
            .handoff(&waiting.id, "child-b", &owner.cancel)
            .is_err()
    );
    manager
        .handoff(&waiting.id, "child-a", &owner.cancel)
        .unwrap();
    owner.cancel.cancel();
    manager
        .write(
            &waiting.id,
            "child-a",
            "survived\n",
            true,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        completed(&manager, &waiting.id).await.status,
        ProcessStatus::Exited
    );
    assert_eq!(
        manager
            .output(&waiting.id, "root", 0, 100)
            .await
            .unwrap()
            .text,
        "survived"
    );

    let abandoned = context("child-a");
    let pending = manager
        .start(command("sleep 30", root.path().into()), &abandoned)
        .await
        .unwrap();
    abandoned.cancel.cancel();
    assert!(
        manager
            .handoff(&pending.id, "child-a", &abandoned.cancel)
            .is_err()
    );
    assert_eq!(
        completed(&manager, &pending.id).await.status,
        ProcessStatus::Cancelled
    );

    let owner = context("child-a");
    let owned = manager
        .start(command("sleep 30", root.path().into()), &owner)
        .await
        .unwrap();
    manager
        .handoff(&owned.id, "child-a", &owner.cancel)
        .unwrap();
    owner.cancel.cancel();
    manager.interrupt_agents(&["child-a".into()]).await;
    assert_eq!(
        completed(&manager, &owned.id).await.status,
        ProcessStatus::Cancelled
    );

    let owner = context("root");
    let owned = manager
        .start(command("sleep 30", root.path().into()), &owner)
        .await
        .unwrap();
    manager.handoff(&owned.id, "root", &owner.cancel).unwrap();
    manager.close().await;
    assert_eq!(
        completed(&manager, &owned.id).await.status,
        ProcessStatus::Cancelled
    );
}
