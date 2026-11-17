//! Restart the app once active turns have finished, if requested. Agent CLI
//! updates are a separate action and never run as part of an app restart.

use std::time::Duration;

use gpui::{App, Context};

use super::Shell;

const TICK: Duration = Duration::from_secs(1);
/// Idle ticks before trusting that agents finished: one agent's turn can end
/// a moment before its next one starts.
const IDLE_TICKS: u32 = 3;
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    Restart,
}

#[derive(Debug)]
pub(super) struct RestartPlan {
    wait_for_agents: bool,
    quiet_ticks: u32,
}

impl RestartPlan {
    fn new(wait_for_agents: bool) -> Self {
        Self {
            wait_for_agents,
            quiet_ticks: 0,
        }
    }

    fn tick(&mut self, blockers: usize) -> Step {
        if self.wait_for_agents {
            if blockers > 0 {
                self.quiet_ticks = 0;
                return Step::Wait;
            }
            self.quiet_ticks += 1;
            if self.quiet_ticks < IDLE_TICKS {
                return Step::Wait;
            }
        }
        Step::Restart
    }
}

impl Shell {
    /// Agents a restart would interrupt: this process's engine only, counting
    /// a chat waiting on the user (restarting would pull the question out from
    /// under them) but not background work, which stops with the restart.
    pub(super) fn restart_blockers(&self, cx: &App) -> usize {
        self.state
            .read(cx)
            .engine()
            .map_or(0, |engine| engine.interrupted_by_restart())
    }

    /// The pill's Restart: straight through when nothing would be
    /// interrupted, otherwise ask.
    pub(super) fn request_update_restart(&mut self, cx: &mut Context<Self>) {
        if self.update_restart.is_some() || self.restart_blockers(cx) > 0 {
            if let Some(update) = crate::app_update::AppUpdate::global(cx) {
                update.update(cx, |update, cx| update.show_result(cx));
            }
        } else {
            self.begin_update_restart(false, cx);
        }
    }

    pub(super) fn begin_update_restart(&mut self, wait_for_agents: bool, cx: &mut Context<Self>) {
        self.update_restart_task = None;
        self.update_restart = Some(RestartPlan::new(wait_for_agents));
        if !self.tick_update_restart(cx) {
            return;
        }
        self.update_restart_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let running = this
                    .update(cx, |this, cx| this.tick_update_restart(cx))
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));
        cx.notify();
    }

    pub(super) fn cancel_update_restart_plan(&mut self, cx: &mut Context<Self>) {
        self.update_restart = None;
        self.update_restart_task = None;
        cx.notify();
    }

    /// Advance the plan; `false` once it is over.
    fn tick_update_restart(&mut self, cx: &mut Context<Self>) -> bool {
        // An unsaved file would stop the restart to ask about it, which a
        // restart the user isn't watching must not do.
        let blockers = self.restart_blockers(cx) + usize::from(!self.all_file_edits_flushed(cx));
        let Some(plan) = self.update_restart.as_mut() else {
            return false;
        };
        match plan.tick(blockers) {
            Step::Wait => {
                cx.notify();
                true
            }
            Step::Restart => {
                self.update_restart = None;
                let staged = crate::app_update::AppUpdate::global(cx)
                    .and_then(|update| update.read(cx).staged());
                match staged {
                    Some(staged) => self.apply_staged_update(staged, cx),
                    None => crate::toast::error(cx, "The update is no longer ready. Try again."),
                }
                cx.notify();
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_now_skips_straight_to_the_restart_with_nothing_to_update() {
        let mut plan = RestartPlan::new(false);
        assert_eq!(plan.tick(2), Step::Restart);
    }

    #[test]
    fn waits_for_agents_and_starts_over_when_one_begins_again() {
        let mut plan = RestartPlan::new(true);
        assert_eq!(plan.tick(1), Step::Wait);
        assert_eq!(plan.tick(0), Step::Wait);
        assert_eq!(plan.tick(0), Step::Wait);
        // A follow-up turn started: the two quiet ticks no longer count.
        assert_eq!(plan.tick(1), Step::Wait);
        assert_eq!(plan.tick(0), Step::Wait);
        assert_eq!(plan.tick(0), Step::Wait);
        assert_eq!(plan.tick(0), Step::Restart);
    }

    #[test]
    fn restart_now_does_not_wait_for_idle_ticks_or_agent_updates() {
        let mut plan = RestartPlan::new(false);
        assert_eq!(plan.tick(0), Step::Restart);
    }

    #[test]
    fn restart_when_idle_only_waits_for_consecutive_idle_ticks() {
        let mut plan = RestartPlan::new(true);
        for _ in 0..2 {
            assert_eq!(plan.tick(0), Step::Wait);
        }
        assert_eq!(plan.tick(0), Step::Restart);
    }
}
