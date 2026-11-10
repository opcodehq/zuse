//! Read the selected agent's executable once in the background, so its first
//! start doesn't wait on a freshly restored disk. Providers that restore disks
//! lazily (E2B, Boat) otherwise spend seconds paging in a ~300 MB binary at
//! the first prompt; here that read overlaps credentials and the checkout.
use std::{io::Read, path::Path, time::Instant};

/// The environment variable the harness release sets for `agent`.
fn executable_var(agent: &str) -> &'static str {
    match agent {
        "codex" => "CODEX_EXECUTABLE",
        "grok" => "GROK_EXECUTABLE",
        _ => "CLAUDE_CODE_EXECUTABLE",
    }
}

/// Where the selected agent's executable is recorded for the boot supervisor,
/// which starts reading it before this process exists on the next restart.
const RECORD: &str = "cloud-agent-executable";

pub fn spawn(agent: &str, data: &Path) {
    let Some(path) = std::env::var_os(executable_var(agent)).filter(|p| !p.is_empty()) else {
        return;
    };
    let _ = std::fs::write(
        data.join(RECORD),
        Path::new(&path).as_os_str().as_encoded_bytes(),
    );
    let agent = agent.to_owned();
    let _ = std::thread::Builder::new()
        .name("agent-warmup".into())
        .spawn(move || {
            let started = Instant::now();
            match read_through(Path::new(&path)) {
                Ok(bytes) => tracing::info!(
                    agent,
                    bytes,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "Agent executable warmed"
                ),
                Err(error) => tracing::debug!(agent, %error, "Agent executable warm-up skipped"),
            }
        });
}

/// Sequentially read a file and discard it; the page cache keeps the bytes.
fn read_through(path: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0; 1 << 20];
    let mut total = 0;
    loop {
        match file.read(&mut buffer)? {
            0 => return Ok(total),
            n => total += n as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_whole_file_and_maps_each_agent_to_its_release_variable() {
        let dir = std::env::temp_dir().join(format!("agent-warmup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent");
        std::fs::write(&path, vec![7u8; (1 << 20) + 5]).unwrap();
        assert_eq!(super::read_through(&path).unwrap(), (1 << 20) + 5);
        assert!(super::read_through(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(super::executable_var("codex"), "CODEX_EXECUTABLE");
        assert_eq!(super::executable_var("grok"), "GROK_EXECUTABLE");
        assert_eq!(super::executable_var("claude"), "CLAUDE_CODE_EXECUTABLE");
        assert_eq!(
            super::executable_var("claude-code"),
            "CLAUDE_CODE_EXECUTABLE"
        );
    }
}
