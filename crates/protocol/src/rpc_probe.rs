//! Direct browser links use a reserved, read-only EngineInfo RPC to detect
//! blackholed connections. Paired and managed links have their own controls.
pub const REQUEST: &str = r#"{"id":0,"method":"EngineInfo","params":{}}"#;
const INTERVAL_MS: u64 = 20_000;
const TIMEOUT_MS: u64 = 60_000;

pub enum Action {
    Send,
    Wait(u64),
    Expired,
}
pub struct Probe {
    next_at: u64,
    deadline: Option<u64>,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            next_at: INTERVAL_MS,
            deadline: None,
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
        self.deadline = Some(now.saturating_add(TIMEOUT_MS));
        Action::Send
    }
    pub fn acknowledge(&mut self, now: u64) {
        if self.deadline.is_some_and(|deadline| now < deadline) {
            self.deadline = None;
            self.next_at = now.saturating_add(INTERVAL_MS);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canceled_reads_and_ordinary_traffic_cannot_postpone_a_lost_probe() {
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
}
