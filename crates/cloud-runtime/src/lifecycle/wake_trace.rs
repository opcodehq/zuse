//! Bounded, best-effort local wake timings; no credentials or HTTP traffic.
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) struct Trace {
    directory: PathBuf,
    binding: Value,
    started: Instant,
    events: Vec<Value>,
}
impl Trace {
    pub(super) fn start(
        directory: &Path,
        workspace: &str,
        operation: &str,
        generation: i64,
        disk: &str,
    ) -> Option<Self> {
        if generation < 1
            || [workspace, operation, disk].iter().any(|s| {
                s.is_empty()
                    || s.len() > 128
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            })
        {
            return None;
        }
        let meta = fs::symlink_metadata(directory).ok()?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
        {
            return None;
        }
        let mut trace = Self {
            directory: directory.into(),
            binding: json!({"workspaceId":workspace,"operationId":operation,"generation":generation,"storageIncarnationId":disk}),
            started: Instant::now(),
            events: Vec::new(),
        };
        trace.record("wake-observed");
        Some(trace)
    }
    pub(super) fn record(&mut self, stage: &'static str) {
        const STAGES: &[&str] = &[
            "wake-observed",
            "authorize-start",
            "authorized",
            "grant-start",
            "granted",
            "grant-not-configured",
            "transport-not-required",
            "transport-reset-start",
            "transport-reset-complete",
            "resumed-ack-start",
            "resumed-acknowledged",
            "core-reopened",
            "authorization-not-admitted",
            "connection-not-admitted",
            "transport-recovery-required",
        ];
        if self.events.len() >= 16 || !STAGES.contains(&stage) {
            return;
        }
        self.events.push(json!({"stage":stage,"elapsedMs":self.started.elapsed().as_millis().min(u64::MAX as u128) as u64}));
        let body = json!({"version":1,"binding":self.binding,"events":self.events});
        let Ok(bytes) = serde_json::to_vec(&body) else {
            return;
        };
        if bytes.len() > 8192 {
            return;
        }
        let temporary = self.directory.join(format!(
            ".resume-trace-{}-{}.next",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            // Diagnostic data need not fsync each stage on the measured critical path.
            // Rename replaces a preexisting symlink/hardlink entry without following it.
            fs::rename(&temporary, self.directory.join("resume-timing.json"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
    }
}
pub(super) fn record(trace: &mut Option<Trace>, stage: &'static str) {
    if let Some(trace) = trace {
        trace.record(stage)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn root() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wake-trace-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }
    #[test]
    fn stages_are_bounded_private_monotonic_and_do_not_contain_credentials() {
        let p = root();
        let mut trace = Trace::start(&p, "workspace", "operation", 1, "disk").unwrap();
        for stage in [
            "authorize-start",
            "authorized",
            "grant-start",
            "granted",
            "transport-reset-start",
            "transport-reset-complete",
            "resumed-ack-start",
            "resumed-acknowledged",
            "core-reopened",
        ] {
            trace.record(stage)
        }
        let v: Value =
            serde_json::from_slice(&fs::read(p.join("resume-timing.json")).unwrap()).unwrap();
        assert_eq!(v["events"].as_array().unwrap().len(), 10);
        assert_eq!(v["binding"].as_object().unwrap().len(), 4);
        let times: Vec<_> = v["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["elapsedMs"].as_u64().unwrap())
            .collect();
        assert!(times.windows(2).all(|v| v[0] <= v[1]));
        assert_eq!(
            fs::metadata(p.join("resume-timing.json")).unwrap().mode() & 0o777,
            0o600
        );
        for _ in 0..100 {
            trace.record("granted")
        }
        assert!(fs::metadata(p.join("resume-timing.json")).unwrap().len() < 8192);
        assert_eq!(trace.events.len(), 16);
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn existing_symlink_is_replaced_without_touching_target() {
        let p = root();
        let target = p.join("unrelated");
        fs::write(&target, b"preserve unrelated bytes").unwrap();
        symlink(&target, p.join("resume-timing.json")).unwrap();
        let _trace = Trace::start(&p, "workspace", "operation", 1, "disk").unwrap();
        assert_eq!(fs::read(target).unwrap(), b"preserve unrelated bytes");
        assert!(
            !fs::symlink_metadata(p.join("resume-timing.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn untrusted_directory_or_identifier_disables_only_diagnostics() {
        let p = root();
        assert!(Trace::start(&p, "workspace", "bad\nidentifier", 1, "disk").is_none());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Trace::start(&p, "workspace", "operation", 1, "disk").is_none());
        fs::remove_dir_all(p).unwrap();
    }
}
