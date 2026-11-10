//! Foreground admission and held queueing have different wake intentions.
//! A foreground message authorizes resume; saving held input never does.
pub(super) fn intent(queue: bool, _sleeping: bool) -> &'static str {
    if queue {
        "preserve-sleep"
    } else {
        "explicit-resume"
    }
}
#[cfg(test)]
mod tests {
    use super::intent;
    #[test]
    fn foreground_send_wakes_even_when_retired_transport_hides_sleep() {
        assert_eq!(intent(false, false), "explicit-resume");
    }
    #[test]
    fn held_queue_never_wakes_a_sleeping_computer() {
        assert_eq!(intent(true, true), "preserve-sleep");
    }
}
