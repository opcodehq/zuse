//! Explicit response playback using the computer's speech service.
use gpui::{App, Global, Task};
use std::{path::Path, process::Stdio};

#[derive(Default)]
struct Speech {
    chat: Option<String>,
    generation: u64,
    task: Option<Task<()>>,
}
impl Global for Speech {}
fn program() -> Option<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") && Path::new("/usr/bin/say").is_file() {
        Some(("/usr/bin/say", &["-f", "-"]))
    } else if cfg!(target_os = "linux") && Path::new("/usr/bin/espeak").is_file() {
        Some(("/usr/bin/espeak", &["--stdin"]))
    } else {
        None
    }
}
pub fn available() -> bool {
    program().is_some()
}
pub fn speaking(cx: &App) -> bool {
    cx.try_global::<Speech>()
        .is_some_and(|speech| speech.chat.is_some())
}
pub fn retain_chat(chat: Option<&str>, cx: &mut App) {
    if cx.try_global::<Speech>().is_some_and(|speech| {
        speech
            .chat
            .as_deref()
            .is_some_and(|owner| Some(owner) != chat)
    }) {
        stop(cx);
    }
}
pub fn stop(cx: &mut App) {
    if !cx.has_global::<Speech>() {
        return;
    }
    let speech = cx.global_mut::<Speech>();
    speech.generation = speech.generation.wrapping_add(1);
    speech.chat = None;
    let task = speech.task.take();
    drop(task);
    cx.refresh_windows();
}
pub fn toggle(chat: String, text: String, cx: &mut App) {
    if speaking(cx) {
        stop(cx);
        return;
    }
    let Some((command, args)) = program() else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    if !cx.has_global::<Speech>() {
        cx.set_global(Speech::default());
    }
    let speech = cx.global_mut::<Speech>();
    speech.generation = speech.generation.wrapping_add(1);
    let generation = speech.generation;
    speech.chat = Some(chat);
    let job = gpui_tokio::Tokio::spawn(cx, async move { play(command, args, &text).await });
    let task = cx.spawn(async move |cx| {
        let result = job.await;
        cx.update(|cx| {
            if cx.global::<Speech>().generation != generation {
                return;
            }
            let speech = cx.global_mut::<Speech>();
            speech.chat = None;
            speech.task = None;
            if !matches!(result, Ok(Ok(()))) {
                crate::toast::error(
                    cx,
                    "Could not play this response. Check your system speech settings.",
                );
            }
            cx.refresh_windows();
        });
    });
    cx.global_mut::<Speech>().task = Some(task);
    cx.refresh_windows();
}

async fn play(command: &str, args: &[&str], text: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(command)
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().expect("piped speech input");
    input.write_all(text.as_bytes()).await?;
    drop(input);
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("Speech service stopped"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn response_text_is_literal_stdin_not_a_command() {
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("spoken.txt");
        let unexpected = tmp.path().join("must-not-exist");
        let text = format!(
            "Hello \"friend\". $(touch {}) `echo not-a-command`",
            unexpected.display()
        );
        let script = "import pathlib,sys; pathlib.Path(sys.argv[1]).write_text(sys.stdin.read())";
        play(
            "/usr/bin/python3",
            &["-c", script, output.to_str().unwrap()],
            &text,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(output).unwrap(), text);
        assert!(!unexpected.exists());
    }
    #[tokio::test]
    async fn stopping_playback_terminates_the_speech_process() {
        let tmp = tempfile::tempdir().unwrap();
        let pid = tmp.path().join("speaker.pid");
        let script = "import os,pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); sys.stdin.read(); time.sleep(30)";
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            play(
                "/usr/bin/python3",
                &["-c", script, pid.to_str().unwrap()],
                "Sample response",
            ),
        )
        .await;
        assert!(result.is_err());
        let pid = std::fs::read_to_string(pid).unwrap();
        let mut stopped = false;
        for _ in 0..20 {
            let alive = tokio::process::Command::new("/bin/kill")
                .args(["-0", &pid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .unwrap()
                .success();
            if !alive {
                stopped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            stopped,
            "Stopping playback must not leave the speaker running"
        );
    }
}
