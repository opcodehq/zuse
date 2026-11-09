//! Metadata-only monotonic phase timings. Dropped/failed phases are never successes.
use std::time::Instant;
pub struct Phase<'a> {
    workspace: &'a str,
    name: &'static str,
    started: Instant,
    succeeded: bool,
}
impl<'a> Phase<'a> {
    pub fn new(workspace: &'a str, name: &'static str) -> Self {
        Self {
            workspace,
            name,
            started: Instant::now(),
            succeeded: false,
        }
    }
    pub fn complete(&mut self) {
        self.succeeded = true;
    }
}
impl Drop for Phase<'_> {
    fn drop(&mut self) {
        tracing::info!(workspace=%self.workspace, phase=self.name,
            duration_ms=self.started.elapsed().as_millis() as u64, succeeded=self.succeeded,
            "Cloud startup phase");
    }
}
