//! A reversible execution barrier. Closing admission never kills retained processes.
use crate::EngineError;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::Notify;
tokio::task_local! { static SCOPES:Vec<usize>; }

#[derive(Default)]
struct State {
    closed: bool,
    epoch: u64,
    active: usize,
}
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    changed: Notify,
}
#[derive(Clone, Default)]
pub struct ExecutionAdmission {
    inner: Arc<Inner>,
}
pub struct ExecutionPermit {
    inner: Arc<Inner>,
}
/// Dropping this token leaves execution closed. Reopening is explicit and scoped
/// to the same engine, after the caller has renewed its runtime authority.
pub struct ExecutionDrain {
    inner: Arc<Inner>,
    epoch: u64,
}
fn state(inner: &Inner) -> MutexGuard<'_, State> {
    inner
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn parked() -> EngineError {
    EngineError::Other("runtime_execution_parked".into())
}
impl ExecutionAdmission {
    pub fn enter(&self) -> Result<ExecutionPermit, EngineError> {
        let mut s = state(&self.inner);
        // Nested execution is part of the already admitted operation. Tokio
        // task-local scope is not inherited by newly spawned/background tasks.
        let key = Arc::as_ptr(&self.inner) as usize;
        let nested = SCOPES
            .try_with(|scopes| scopes.contains(&key))
            .unwrap_or(false);
        if s.closed && !nested {
            return Err(parked());
        }
        s.active = s.active.checked_add(1).ok_or_else(parked)?;
        Ok(ExecutionPermit {
            inner: self.inner.clone(),
        })
    }
    pub fn is_closed(&self) -> bool {
        state(&self.inner).closed
    }
    pub fn close(&self) -> Result<ExecutionDrain, EngineError> {
        let mut s = state(&self.inner);
        if s.closed {
            return Err(parked());
        }
        s.epoch = s.epoch.checked_add(1).ok_or_else(parked)?;
        s.closed = true;
        Ok(ExecutionDrain {
            inner: self.inner.clone(),
            epoch: s.epoch,
        })
    }
}
impl ExecutionPermit {
    pub(crate) async fn scope<F: std::future::Future>(self, future: F) -> F::Output {
        let mut scopes = SCOPES.try_with(Clone::clone).unwrap_or_default();
        scopes.push(Arc::as_ptr(&self.inner) as usize);
        let result = SCOPES.scope(scopes, future).await;
        drop(self);
        result
    }
}
impl Drop for ExecutionPermit {
    fn drop(&mut self) {
        let mut s = state(&self.inner);
        s.active = s
            .active
            .checked_sub(1)
            .expect("execution permit belongs to one admission");
        if s.active == 0 {
            self.inner.changed.notify_waiters();
        }
    }
}
impl ExecutionDrain {
    pub async fn drained(&self) {
        loop {
            let notification = self.inner.changed.notified();
            tokio::pin!(notification);
            notification.as_mut().enable();
            if state(&self.inner).active == 0 {
                return;
            }
            notification.await;
        }
    }
    pub fn reopen(self, admission: &ExecutionAdmission) -> Result<(), EngineError> {
        if !Arc::ptr_eq(&self.inner, &admission.inner) {
            return Err(parked());
        }
        let mut s = state(&self.inner);
        if !s.closed || s.epoch != self.epoch || s.active != 0 {
            return Err(parked());
        }
        s.closed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn close_rejects_racing_mutations_and_waits_for_every_admitted_operation() {
        let gate = ExecutionAdmission::default();
        let one = gate.enter().unwrap();
        let two = gate.enter().unwrap();
        let drain = gate.close().unwrap();
        let contenders = (0..64).map(|_| {
            let gate = gate.clone();
            tokio::spawn(async move {
                assert!(gate.enter().is_err());
            })
        });
        for contender in contenders {
            contender.await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(5), drain.drained())
                .await
                .is_err()
        );
        drop(one);
        assert!(
            tokio::time::timeout(Duration::from_millis(5), drain.drained())
                .await
                .is_err()
        );
        drop(two);
        tokio::time::timeout(Duration::from_secs(1), drain.drained())
            .await
            .unwrap();
        drain.reopen(&gate).unwrap();
        assert!(gate.enter().is_ok());
    }
    #[tokio::test]
    async fn admitted_nested_calls_settle_but_new_tasks_do_not_inherit_authority() {
        let gate = ExecutionAdmission::default();
        let permit = gate.enter().unwrap();
        let drain = permit
            .scope(async {
                let drain = gate.close().unwrap();
                let nested = gate
                    .enter()
                    .expect("a pre-admitted operation can finish a nested dispatch");
                let unrelated = gate.clone();
                assert!(
                    tokio::spawn(async move { unrelated.enter().is_err() })
                        .await
                        .unwrap(),
                    "spawned work needs a new admission"
                );
                drop(nested);
                assert!(
                    tokio::time::timeout(Duration::from_millis(5), drain.drained())
                        .await
                        .is_err()
                );
                drain
            })
            .await;
        drain.drained().await;
        assert!(
            gate.enter().is_err(),
            "the task-local admission ended with its permit"
        );
        drain.reopen(&gate).unwrap();
    }

    #[tokio::test]
    async fn cancelled_parking_and_a_foreign_engine_cannot_reopen_admission() {
        let gate = ExecutionAdmission::default();
        let permit = gate.enter().unwrap();
        let drain = gate.close().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), drain.drained())
                .await
                .is_err()
        );
        drop(drain);
        drop(permit);
        assert!(gate.enter().is_err());
        let other = ExecutionAdmission::default();
        let drain = other.close().unwrap();
        assert!(drain.reopen(&gate).is_err());
        assert!(other.enter().is_err());
    }
    #[tokio::test]
    async fn a_new_close_cannot_be_overtaken_by_a_previous_drain_notification() {
        let gate = ExecutionAdmission::default();
        for _ in 0..128 {
            let permit = gate.enter().unwrap();
            let drain = gate.close().unwrap();
            drop(permit);
            drain.drained().await;
            drain.reopen(&gate).unwrap();
        }
        let permit = gate.enter().unwrap();
        let drain = gate.close().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), drain.drained())
                .await
                .is_err()
        );
        drop(permit);
        drain.drained().await;
        drain.reopen(&gate).unwrap();
    }
}
