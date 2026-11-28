//! When a cloud chat's live connection to its computer (files, terminal and
//! the live transcript) cannot be opened. Failures retry quietly with
//! backoff; only a failure that lasts is shown, with a Reconnect that tries
//! at once. The chat itself is saved on the server and its work keeps running
//! on the computer either way.
use std::time::{Duration, Instant, SystemTime};

/// The strip shown once the connection has stayed down.
pub(super) const NOTICE: &str =
    "Can't reach the computer right now. The chat is saved and work keeps running there.";
/// A failure is shown only after the connection has been failing this long.
const SHOW_AFTER: Duration = Duration::from_secs(20);
const FIRST_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(30);
/// How often a wait checks the wall clock for a sleep of the machine.
const TICK: Duration = Duration::from_secs(1);
/// A tick that lands this much later on the wall clock means the machine
/// slept (or its network was down long enough to stall it): try at once.
const WAKE_GAP: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Backoff {
    failures: u32,
    failing_since: Option<Instant>,
    shown: bool,
}
impl Backoff {
    /// Record a failed attempt; returns how long to wait before the next.
    /// A failed Reconnect the user asked for is shown at once.
    pub(super) fn failed(&mut self, now: Instant, manual: bool) -> Duration {
        self.failures = self.failures.saturating_add(1);
        let since = *self.failing_since.get_or_insert(now);
        if manual || now.saturating_duration_since(since) >= SHOW_AFTER {
            self.shown = true;
        }
        delay(self.failures)
    }
    pub(super) fn succeeded(&mut self) {
        *self = Self::default();
    }
    pub(super) fn visible(&self) -> bool {
        self.shown
    }
    #[cfg(test)]
    pub(super) fn failing(&self) -> bool {
        self.failures > 0
    }
}
fn delay(failures: u32) -> Duration {
    (FIRST_DELAY * (1u32 << failures.saturating_sub(1).min(5))).min(MAX_DELAY)
}
/// Whether a tick scheduled for `scheduled` took `wall` on the wall clock
/// because the machine slept in between.
fn woke(scheduled: Duration, wall: Option<Duration>) -> bool {
    wall.is_some_and(|wall| wall >= scheduled + WAKE_GAP)
}
/// Wait `delay` before the next attempt, ending early when the machine wakes
/// from sleep: its connection is worth trying again at once.
pub(super) async fn wait(executor: gpui::BackgroundExecutor, delay: Duration) {
    let mut left = delay;
    while !left.is_zero() {
        let tick = TICK.min(left);
        let started = SystemTime::now();
        executor.timer(tick).await;
        if woke(tick, started.elapsed().ok()) {
            return;
        }
        left = left.saturating_sub(tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transient_failures_retry_quietly_and_a_lasting_one_is_shown() {
        let start = Instant::now();
        let mut backoff = Backoff::default();
        assert_eq!(backoff.failed(start, false), Duration::from_secs(1));
        assert_eq!(
            backoff.failed(start + Duration::from_secs(1), false),
            Duration::from_secs(2)
        );
        assert!(!backoff.visible(), "a brief failure stays silent");
        assert_eq!(
            backoff.failed(start + Duration::from_secs(3), false),
            Duration::from_secs(4)
        );
        backoff.failed(start + Duration::from_secs(7), false);
        backoff.failed(start + Duration::from_secs(15), false);
        assert!(!backoff.visible());
        assert_eq!(
            backoff.failed(start + Duration::from_secs(31), false),
            Duration::from_secs(30),
            "the wait is capped"
        );
        assert!(backoff.visible(), "a failure lasting 20s is shown");
        assert_eq!(
            backoff.failed(start + Duration::from_secs(61), false),
            MAX_DELAY
        );
        backoff.succeeded();
        assert!(!backoff.visible() && !backoff.failing());
        assert_eq!(backoff.failed(start, false), FIRST_DELAY);
    }
    #[test]
    fn a_failed_reconnect_the_user_asked_for_is_shown_at_once() {
        let mut backoff = Backoff::default();
        backoff.failed(Instant::now(), true);
        assert!(backoff.visible());
    }
    #[test]
    fn a_wall_clock_jump_reads_as_a_wake() {
        assert!(!woke(TICK, Some(TICK)));
        assert!(!woke(TICK, Some(Duration::from_secs(3))));
        assert!(woke(TICK, Some(Duration::from_secs(600))));
        // A clock set backwards is not a wake.
        assert!(!woke(TICK, None));
    }
}
