//! Wakeups for background loops.
//!
//! Loops sleep until the earliest of:
//! - a PostgreSQL `NOTIFY` saying matching work was created or rescheduled
//!   (triggers from migrations `0043_work_wakeups.sql` and
//!   `0045_auth_wakeups.sql`),
//! - the next known due time of their work (`due_at`, `next_attempt_at`,
//!   lease expiry), or
//! - an idle fallback timer.
//!
//! `NOTIFY` is only a latency optimisation. A lost notification, a dropped
//! `LISTEN` connection or `ZUSE_CLOUD_DB_LISTEN=false` only delays work until
//! its due time or the idle timer; correctness never depends on delivery.
use super::Budget;
use std::{sync::Arc, time::Duration};
use tokio::sync::broadcast;

/// Channel prefix; the schema name follows so staging and production sharing
/// one database never wake each other.
pub const CHANNEL_PREFIX: &str = "zuse_cloud_work:";

/// What kind of work a notification announces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Jobs,
    TaskDrain,
    Recreation,
    MachineImages,
    ProjectCache,
    AccountEvents,
    /// A command mailbox revision changed.
    Mailbox,
    /// Credential worker: a sign-in or proposal became claimable, a refresh
    /// started, or a refresh result went stale (migration 0045).
    Auth,
    /// An account's shared-credit hold turned on; its online computers should
    /// be paused (trigger from optional migration `0047_billing_wakeups.sql`).
    IdleSleep,
    /// A plugin connection or custom server row changed (migration
    /// `0046_plugin_wakeups.sql`); cached plugin views must be re-read.
    Plugins,
    /// An archived computer's deletion due time appeared or moved earlier
    /// (migration `0050_archive_machine_deletion.sql`).
    ArchiveRetention,
    /// A computer request was made, decided, claimed, finished or stopped
    /// (sent by the API with its `computer-request` hint); bridge long-polls
    /// re-read.
    Bridge,
    /// Notifications may have been missed (listener reconnected or lagged).
    All,
}

impl Work {
    pub fn parse(payload: &str) -> Option<Self> {
        Some(match payload {
            "jobs" => Self::Jobs,
            "task_drain" => Self::TaskDrain,
            "recreation" => Self::Recreation,
            "machine_images" => Self::MachineImages,
            "project_cache" => Self::ProjectCache,
            "account_events" => Self::AccountEvents,
            "mailbox" => Self::Mailbox,
            "auth" => Self::Auth,
            "idle_sleep" => Self::IdleSleep,
            "plugins" => Self::Plugins,
            "archive_retention" => Self::ArchiveRetention,
            "bridge" => Self::Bridge,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jobs => "jobs",
            Self::TaskDrain => "task_drain",
            Self::Recreation => "recreation",
            Self::MachineImages => "machine_images",
            Self::ProjectCache => "project_cache",
            Self::AccountEvents => "account_events",
            Self::Mailbox => "mailbox",
            Self::Auth => "auth",
            Self::IdleSleep => "idle_sleep",
            Self::Plugins => "plugins",
            Self::ArchiveRetention => "archive_retention",
            Self::Bridge => "bridge",
            Self::All => "all",
        }
    }
    fn matches(self, interest: &[Work]) -> bool {
        self == Self::All || interest.contains(&self)
    }
}

/// The channel name for one schema, truncated like PostgreSQL truncates identifiers.
pub fn channel(schema: &str) -> String {
    let mut name = format!("{CHANNEL_PREFIX}{schema}");
    while name.len() > 63 {
        name.pop();
    }
    name
}

/// Fan-out of database notifications to the loops of one process.
pub struct Wakeups {
    sender: broadcast::Sender<Work>,
    enabled: bool,
}

impl Wakeups {
    /// No `LISTEN` connection: loops rely on due times and the idle timer.
    pub fn disabled() -> Arc<Self> {
        Arc::new(Self {
            sender: broadcast::channel(1).0,
            enabled: false,
        })
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Work> {
        self.sender.subscribe()
    }
    /// In-process wake for loops of this process (also used by tests).
    pub fn notify(&self, work: Work) {
        let _ = self.sender.send(work);
    }
    /// Opens one dedicated connection (`<role>-listen`) and forwards
    /// notifications for this schema until the process exits.
    pub async fn listen(url: &str, budget: &Budget) -> anyhow::Result<Arc<Self>> {
        let options =
            super::connect_options(url, &format!("zuse-cloud-{}-listen", budget.role.as_str()))?;
        // The listener's private pool only reconnects; it never serves queries.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .acquire_timeout(budget.acquire_timeout)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        Self::listen_with(pool).await
    }
    /// [`Self::listen`] on a caller-provided single-connection pool.
    pub async fn listen_with(pool: sqlx::PgPool) -> anyhow::Result<Arc<Self>> {
        let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
        listener.ignore_pool_close_event(true);
        let schema: String = sqlx::query_scalar("SELECT current_schema()")
            .fetch_one(&mut listener)
            .await?;
        let channel = channel(&schema);
        listener.listen(&channel).await?;
        let wakeups = Arc::new(Self {
            sender: broadcast::channel(256).0,
            enabled: true,
        });
        let weak = Arc::downgrade(&wakeups);
        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                let received = listener.try_recv().await;
                let Some(wakeups) = weak.upgrade() else { break };
                match received {
                    Ok(Some(notification)) => {
                        failures = 0;
                        match Work::parse(notification.payload()) {
                            Some(work) => wakeups.notify(work),
                            None => tracing::debug!(
                                payload = notification.payload(),
                                "Ignored unknown database wakeup"
                            ),
                        }
                    }
                    Ok(None) => {
                        // Reconnected: anything sent meanwhile was lost.
                        tracing::warn!(
                            code = "database_listen_reconnected",
                            "Database wakeups reconnected"
                        );
                        wakeups.notify(Work::All);
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        let wait = super::failure_backoff(
                            failures,
                            Duration::from_secs(1),
                            Duration::from_secs(30),
                        );
                        tracing::warn!(
                            code = "database_listen_failed",
                            error = %error,
                            retry_ms = wait.as_millis() as u64,
                            "Database wakeups interrupted"
                        );
                        drop(wakeups);
                        tokio::time::sleep(wait).await;
                        if let Some(wakeups) = weak.upgrade() {
                            wakeups.notify(Work::All);
                        }
                    }
                }
            }
        });
        Ok(wakeups)
    }
}

/// Why a loop woke up; logged at debug level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Notify(Work),
    /// Known work became due.
    Due,
    /// Nothing known is due; periodic safety poll.
    Idle,
    /// A same-host hint or in-process signal.
    Hint,
    Shutdown,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Notify(_) => "notify",
            Self::Due => "due",
            Self::Idle => "idle",
            Self::Hint => "hint",
            Self::Shutdown => "shutdown",
        }
    }
}

/// Cadence of one sleeping loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Longest sleep when nothing is known to be due.
    pub idle: Duration,
    /// Shortest sleep when work stays due after a round (it could not be
    /// claimed, for example while another owner runs). Bounds any spin.
    pub busy: Duration,
}

impl Cadence {
    /// `ZUSE_CLOUD_WORKER_IDLE_POLL_MS` (default 5 minutes with `LISTEN`, 5s
    /// without) and `ZUSE_CLOUD_WORKER_POLL_MS` (default 1s) for the busy floor.
    ///
    /// With `LISTEN` every work source sends `NOTIFY` and every timed transition
    /// is a known due time, so the idle timer is only a safety net for a lost
    /// notification.
    pub fn from_env(listening: bool, busy_ms: u64) -> anyhow::Result<Self> {
        Self::from_lookup(listening, busy_ms, |name| std::env::var(name).ok())
    }
    pub fn from_lookup(
        listening: bool,
        busy_ms: u64,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Self> {
        let idle_ms =
            match lookup("ZUSE_CLOUD_WORKER_IDLE_POLL_MS").filter(|v| !v.trim().is_empty()) {
                None if listening => IDLE_WITH_LISTEN_MS,
                None => 5_000,
                Some(raw) => raw
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|ms| (1_000..=300_000).contains(ms))
                    .ok_or_else(|| anyhow::anyhow!("Invalid ZUSE_CLOUD_WORKER_IDLE_POLL_MS"))?,
            };
        anyhow::ensure!(
            (50..=60_000).contains(&busy_ms),
            "Invalid ZUSE_CLOUD_WORKER_POLL_MS"
        );
        Ok(Self {
            idle: Duration::from_millis(idle_ms),
            busy: Duration::from_millis(busy_ms.min(idle_ms)),
        })
    }
    /// Longest backoff after failed database rounds. Kept short so a loop
    /// recovers quickly from an outage even when its idle timer is long.
    pub fn failure_cap(&self) -> Duration {
        self.idle.min(Duration::from_millis(FAILURE_CAP_MS))
    }
}

/// Idle safety net when `LISTEN` is enabled.
pub const IDLE_WITH_LISTEN_MS: u64 = 300_000;
/// Longest backoff after consecutive database failures.
pub const FAILURE_CAP_MS: u64 = 20_000;

/// How long to sleep given the earliest known due time (epoch ms).
///
/// - nothing known: the idle fallback;
/// - due in the future: until then, but never longer than the idle fallback;
/// - already due after a full round: the busy floor, so unclaimable work
///   cannot spin.
pub fn next_wait(now_ms: i64, next_due_ms: Option<i64>, cadence: Cadence) -> (Duration, Reason) {
    let Some(due) = next_due_ms else {
        return (cadence.idle, Reason::Idle);
    };
    let until = due.saturating_sub(now_ms);
    if until <= 0 {
        return (cadence.busy, Reason::Due);
    }
    let until = Duration::from_millis(until as u64);
    if until >= cadence.idle {
        (cadence.idle, Reason::Idle)
    } else {
        // Wake slightly after the due instant so the round sees it as due.
        (until + Duration::from_millis(5), Reason::Due)
    }
}

/// The earliest of several optional due times.
pub fn earliest(times: impl IntoIterator<Item = Option<i64>>) -> Option<i64> {
    times.into_iter().flatten().min()
}

/// Waits up to `wait` for a notification matching `interest`. Unrelated
/// notifications keep waiting for the remaining time.
pub async fn sleep_or_notify(
    receiver: &mut broadcast::Receiver<Work>,
    interest: &[Work],
    wait: Duration,
    timer_reason: Reason,
) -> Reason {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return timer_reason,
            received = receiver.recv() => match received {
                Ok(work) if work.matches(interest) => return Reason::Notify(work),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => return Reason::Notify(Work::All),
                // Disabled or shut down: only the timer remains.
                Err(broadcast::error::RecvError::Closed) => {
                    tokio::time::sleep_until(deadline).await;
                    return timer_reason;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CADENCE: Cadence = Cadence {
        idle: Duration::from_secs(20),
        busy: Duration::from_secs(1),
    };

    #[test]
    fn next_wait_sleeps_until_due_bounded_by_idle_and_busy() {
        assert_eq!(
            next_wait(1_000, None, CADENCE),
            (CADENCE.idle, Reason::Idle)
        );
        assert_eq!(
            next_wait(1_000, Some(4_000), CADENCE),
            (Duration::from_millis(3_005), Reason::Due)
        );
        assert_eq!(
            next_wait(1_000, Some(1_000 + 60_000), CADENCE),
            (CADENCE.idle, Reason::Idle)
        );
        // Still due after a round: never spin faster than the busy floor.
        assert_eq!(
            next_wait(1_000, Some(1_000), CADENCE),
            (CADENCE.busy, Reason::Due)
        );
        assert_eq!(
            next_wait(1_000, Some(-5), CADENCE),
            (CADENCE.busy, Reason::Due)
        );
        assert_eq!(
            next_wait(0, Some(i64::MAX), CADENCE),
            (CADENCE.idle, Reason::Idle)
        );
        assert_eq!(next_wait(i64::MAX, Some(i64::MIN), CADENCE).1, Reason::Due);
    }

    #[test]
    fn earliest_ignores_unknown_times() {
        assert_eq!(earliest([None, Some(5), Some(3), None]), Some(3));
        assert_eq!(earliest([None, None]), None);
    }

    #[test]
    fn cadence_defaults_depend_on_listening() {
        let none = |_: &str| None;
        assert_eq!(
            Cadence::from_lookup(true, 1_000, none).unwrap(),
            Cadence {
                idle: Duration::from_secs(300),
                busy: Duration::from_secs(1)
            }
        );
        // Failure backoff stays short even with the long idle safety net.
        assert_eq!(
            Cadence::from_lookup(true, 1_000, none)
                .unwrap()
                .failure_cap(),
            Duration::from_secs(20)
        );
        assert_eq!(
            Cadence::from_lookup(false, 1_000, none)
                .unwrap()
                .failure_cap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            Cadence::from_lookup(false, 1_000, none).unwrap().idle,
            Duration::from_secs(5)
        );
        let custom =
            |name: &str| (name == "ZUSE_CLOUD_WORKER_IDLE_POLL_MS").then(|| "30000".to_owned());
        assert_eq!(
            Cadence::from_lookup(true, 1_000, custom).unwrap().idle,
            Duration::from_secs(30)
        );
        let bad = |_: &str| Some("10".to_owned());
        assert!(Cadence::from_lookup(true, 1_000, bad).is_err());
        assert!(Cadence::from_lookup(true, 0, none).is_err());
        // The busy floor never exceeds the idle fallback.
        let short = |_: &str| Some("2000".to_owned());
        assert_eq!(
            Cadence::from_lookup(true, 5_000, short).unwrap().busy,
            Duration::from_secs(2)
        );
    }

    #[test]
    fn channel_names_are_schema_scoped_and_bounded() {
        assert_eq!(
            channel("zuse_production"),
            "zuse_cloud_work:zuse_production"
        );
        assert_eq!(channel(&"s".repeat(80)).len(), 63);
        for kind in [
            Work::Jobs,
            Work::TaskDrain,
            Work::Recreation,
            Work::MachineImages,
            Work::ProjectCache,
            Work::AccountEvents,
            Work::Mailbox,
            Work::Auth,
            Work::IdleSleep,
            Work::Plugins,
            Work::ArchiveRetention,
            Work::Bridge,
        ] {
            assert_eq!(Work::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Work::parse("all"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_or_notify_filters_by_interest() {
        let wakeups = Wakeups {
            sender: broadcast::channel(4).0,
            enabled: true,
        };
        let mut receiver = wakeups.subscribe();
        wakeups.notify(Work::AccountEvents);
        wakeups.notify(Work::Jobs);
        assert_eq!(
            sleep_or_notify(
                &mut receiver,
                &[Work::Jobs],
                Duration::from_secs(5),
                Reason::Idle
            )
            .await,
            Reason::Notify(Work::Jobs)
        );
        // Unrelated work does not end the sleep early.
        wakeups.notify(Work::AccountEvents);
        let started = tokio::time::Instant::now();
        assert_eq!(
            sleep_or_notify(
                &mut receiver,
                &[Work::Jobs],
                Duration::from_secs(5),
                Reason::Due
            )
            .await,
            Reason::Due
        );
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        // A reconnect wakes every loop.
        wakeups.notify(Work::All);
        assert_eq!(
            sleep_or_notify(
                &mut receiver,
                &[Work::TaskDrain],
                Duration::from_secs(5),
                Reason::Idle
            )
            .await,
            Reason::Notify(Work::All)
        );
        // A disabled waker only ever times out.
        let disabled = Wakeups::disabled();
        let mut receiver = disabled.subscribe();
        drop(disabled);
        assert_eq!(
            sleep_or_notify(
                &mut receiver,
                &[Work::Jobs],
                Duration::from_secs(3),
                Reason::Idle
            )
            .await,
            Reason::Idle
        );
    }
}
