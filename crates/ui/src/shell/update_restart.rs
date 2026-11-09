//! Sequencing an update restart: wait for agents if asked, update the agent
//! CLIs, then restart. An agent CLI that is missing or fails is skipped; only
//! the app's own install can stop the restart.

use std::time::{Duration, Instant};

use gpui::{App, Context};

use super::Shell;

const TICK: Duration = Duration::from_secs(1);
/// Idle ticks before trusting that agents finished: one agent's turn can end
/// a moment before its next one starts.
const IDLE_TICKS: u32 = 3;
/// Settled ticks before restarting, covering the gap between an update's
/// request and its first status.
const SETTLED_TICKS: u32 = 2;
/// A stuck agent update must not hold the app's update hostage.
const AGENT_UPDATE_LIMIT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestartStage {
    WaitingForAgents,
    UpdatingAgents,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    UpdateAgents,
    Restart,
}

#[derive(Debug)]
pub(super) struct RestartPlan {
    wait_for_agents: bool,
    idle_confirmed: bool,
    agent_updates_started: Option<Instant>,
    quiet_ticks: u32,
}

impl RestartPlan {
    fn new(wait_for_agents: bool) -> Self {
        Self {
            wait_for_agents,
            idle_confirmed: false,
            agent_updates_started: None,
            quiet_ticks: 0,
        }
    }

    pub(super) fn stage(&self) -> RestartStage {
        if self.wait_for_agents && !self.idle_confirmed {
            RestartStage::WaitingForAgents
        } else {
            RestartStage::UpdatingAgents
        }
    }

    fn tick(&mut self, blockers: usize, targets: usize, settled: bool, now: Instant) -> Step {
        if self.wait_for_agents {
            if blockers > 0 {
                self.idle_confirmed = false;
                self.quiet_ticks = 0;
                return Step::Wait;
            }
            if !self.idle_confirmed {
                self.quiet_ticks += 1;
                if self.quiet_ticks < IDLE_TICKS {
                    return Step::Wait;
                }
                self.idle_confirmed = true;
                self.quiet_ticks = 0;
            }
        }
        let Some(started) = self.agent_updates_started else {
            if targets == 0 {
                return Step::Restart;
            }
            self.agent_updates_started = Some(now);
            return Step::UpdateAgents;
        };
        if now.saturating_duration_since(started) >= AGENT_UPDATE_LIMIT {
            return Step::Restart;
        }
        if !settled {
            self.quiet_ticks = 0;
            return Step::Wait;
        }
        self.quiet_ticks += 1;
        if self.quiet_ticks >= SETTLED_TICKS {
            Step::Restart
        } else {
            Step::Wait
        }
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
        let targets = self.agent_update_count();
        let settled = self.agent_updates_settled();
        let Some(plan) = self.update_restart.as_mut() else {
            return false;
        };
        match plan.tick(blockers, targets, settled, Instant::now()) {
            Step::Wait => {
                cx.notify();
                true
            }
            Step::UpdateAgents => {
                self.update_all_harnesses(cx);
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
        assert_eq!(plan.tick(2, 0, true, Instant::now()), Step::Restart);
    }

    #[test]
    fn waits_for_agents_and_starts_over_when_one_begins_again() {
        let now = Instant::now();
        let mut plan = RestartPlan::new(true);
        assert_eq!(plan.stage(), RestartStage::WaitingForAgents);
        assert_eq!(plan.tick(1, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Wait);
        // A follow-up turn started: the two quiet ticks no longer count.
        assert_eq!(plan.tick(1, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Restart);
    }

    #[test]
    fn agent_updates_run_once_then_restart_when_settled() {
        let now = Instant::now();
        let mut plan = RestartPlan::new(false);
        assert_eq!(plan.tick(0, 2, false, now), Step::UpdateAgents);
        assert_eq!(plan.stage(), RestartStage::UpdatingAgents);
        assert_eq!(plan.tick(0, 0, false, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Wait);
        assert_eq!(plan.tick(0, 0, true, now), Step::Restart);
    }

    #[test]
    fn a_stuck_agent_update_cannot_block_the_restart() {
        let now = Instant::now();
        let mut plan = RestartPlan::new(false);
        assert_eq!(plan.tick(0, 1, false, now), Step::UpdateAgents);
        assert_eq!(
            plan.tick(0, 0, false, now + AGENT_UPDATE_LIMIT),
            Step::Restart
        );
    }

    #[test]
    fn an_agent_starting_during_agent_updates_holds_a_waiting_restart() {
        let now = Instant::now();
        let mut plan = RestartPlan::new(true);
        for _ in 0..2 {
            assert_eq!(plan.tick(0, 1, false, now), Step::Wait);
        }
        assert_eq!(plan.tick(0, 1, false, now), Step::UpdateAgents);
        assert_eq!(plan.tick(1, 0, true, now), Step::Wait);
        assert_eq!(plan.stage(), RestartStage::WaitingForAgents);
    }
}
