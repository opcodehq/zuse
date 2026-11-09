//! Direct browser links use a reserved, read-only EngineInfo RPC to detect
//! blackholed connections. Paired and managed links have their own controls.
pub const REQUEST: &str = r#"{"id":0,"method":"EngineInfo","params":{}}"#;
const INTERVAL_MS: u64 = 20_000;
const TIMEOUT_MS: u64 = 60_000;
// A failed uplink must still expire even while the host streams other data.
const MAX_PENDING_MS: u64 = 15 * 60_000;
// Allow half the benchmark's 400 kbps downlink rate. Notices describe encoded
// bytes, not decoded JSON, so highly compressible history doesn't over-budget.
const TRANSFER_BYTES_PER_SECOND: u64 = 25_000;
pub const TRANSFER_NOTICE_THRESHOLD: usize = 64 * 1024;
const MAX_TRANSFER_BYTES: u64 = 20 * 1024 * 1024;

pub enum Action {
    Send,
    Wait(u64),
    Expired,
}
pub struct Probe {
    next_at: u64,
    deadline: Option<u64>,
    hard_deadline: u64,
    incoming_until: u64,
    outgoing_until: u64,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            next_at: INTERVAL_MS,
            deadline: None,
            hard_deadline: 0,
            incoming_until: 0,
            outgoing_until: 0,
        }
    }
}
impl Probe {
    pub fn awaiting_reply(&self) -> bool {
        self.deadline.is_some()
    }
    pub fn poll(&mut self, now: u64) -> Action {
        if let Some(deadline) = self.deadline {
            return if now >= deadline {
                Action::Expired
            } else {
                Action::Wait(deadline - now)
            };
        }
        if now < self.next_at {
            return Action::Wait(self.next_at - now);
        }
        self.hard_deadline = now.saturating_add(MAX_PENDING_MS);
        self.deadline = Some(self.allowed_until(now));
        Action::Send
    }
    fn allowed_until(&self, now: u64) -> u64 {
        now.saturating_add(TIMEOUT_MS)
            .max(self.incoming_until)
            .max(self.outgoing_until)
            .min(self.hard_deadline)
    }
    fn transfer_until(now: u64, bytes: u64) -> u64 {
        let transfer_ms = bytes
            .min(MAX_TRANSFER_BYTES)
            .saturating_mul(1000)
            .div_ceil(TRANSFER_BYTES_PER_SECOND);
        now.saturating_add(TIMEOUT_MS).saturating_add(transfer_ms)
    }
    /// An opt-in notice arrives before a large WebSocket frame. Browsers expose
    /// completed messages, not byte progress during that frame's download.
    pub fn incoming_transfer(&mut self, now: u64, encoded_bytes: u64) {
        self.incoming_until = Self::transfer_until(now, encoded_bytes);
        if self.deadline.is_some_and(|deadline| now < deadline) {
            self.deadline = Some(self.allowed_until(now));
        }
    }
    /// The browser's own send buffer can delay the read-only probe too.
    pub fn outgoing_buffer(&mut self, now: u64, bytes: u64) {
        if bytes >= TRANSFER_NOTICE_THRESHOLD as u64 {
            self.outgoing_until = Self::transfer_until(now, bytes);
            if self.deadline.is_some_and(|deadline| now < deadline) {
                self.deadline = Some(self.allowed_until(now));
            }
        }
    }
    /// Completed traffic proves downlink progress, but cannot acknowledge an
    /// uplink probe or extend its absolute lifetime indefinitely.
    pub fn received_frame(&mut self, now: u64) {
        self.incoming_until = 0;
        if self.deadline.is_some_and(|deadline| now < deadline) {
            self.deadline = Some(self.allowed_until(now));
        }
    }
    pub fn acknowledge(&mut self, now: u64) {
        if self.deadline.is_some_and(|deadline| now < deadline) {
            self.deadline = None;
            self.outgoing_until = 0;
            self.incoming_until = 0;
            self.next_at = now.saturating_add(INTERVAL_MS);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canceled_reads_cannot_postpone_a_lost_probe() {
        let mut probe = Probe::default();
        assert!(matches!(probe.poll(0), Action::Wait(20_000)));
        assert!(matches!(probe.poll(20_000), Action::Send));
        for now in [20_001, 30_000, 79_999] {
            assert!(matches!(probe.poll(now), Action::Wait(_)));
        }
        assert!(matches!(probe.poll(80_000), Action::Expired));
        probe.acknowledge(80_000);
        assert!(matches!(probe.poll(80_001), Action::Expired));
    }
    #[test]
    fn only_a_timely_reply_schedules_the_next_read_only_probe() {
        let mut probe = Probe::default();
        assert!(matches!(probe.poll(20_000), Action::Send));
        probe.acknowledge(21_000);
        assert!(matches!(probe.poll(40_999), Action::Wait(1)));
        assert!(matches!(probe.poll(41_000), Action::Send));
    }
    #[test]
    fn slow_large_frames_and_queued_uploads_do_not_expire_healthy_links() {
        let mut probe = Probe::default();
        // Notice precedes the probe: the binary message is already downloading.
        probe.incoming_transfer(10_000, 8 * 1024 * 1024);
        assert!(matches!(probe.poll(20_000), Action::Send));
        assert!(matches!(probe.poll(180_000), Action::Wait(_)));
        probe.received_frame(350_000);
        probe.acknowledge(351_000);
        assert!(matches!(probe.poll(371_000), Action::Send));
        probe.outgoing_buffer(371_000, 8 * 1024 * 1024);
        assert!(matches!(probe.poll(500_000), Action::Wait(_)));
        probe.received_frame(510_000);
        assert!(
            matches!(probe.poll(600_000), Action::Wait(_)),
            "incoming traffic must not discard the upload allowance"
        );
        probe.acknowledge(710_000);
        assert!(matches!(probe.poll(730_000), Action::Send));
    }
    #[test]
    fn continuous_downstream_and_notices_cannot_hide_a_failed_uplink_forever() {
        let mut probe = Probe::default();
        assert!(matches!(probe.poll(20_000), Action::Send));
        for now in (30_000..920_000).step_by(10_000) {
            probe.received_frame(now);
            probe.incoming_transfer(now, u64::MAX);
            assert!(matches!(probe.poll(now), Action::Wait(_)));
        }
        probe.received_frame(920_000);
        probe.incoming_transfer(920_000, u64::MAX);
        assert!(matches!(probe.poll(920_000), Action::Expired));
        probe.acknowledge(920_000);
        assert!(matches!(probe.poll(920_001), Action::Expired));
    }
}
