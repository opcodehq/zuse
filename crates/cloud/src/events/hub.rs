//! Account feed dispatches compact durable invalidations. Execution and tool output
//! stay on task channels; neither subscription nor replay calls a provider.
use super::{cursor::Cursors, store::EventStore, tickets::Tickets};
use crate::{crypto::MasterKey, error::ApiError};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore, watch};

pub const PROTOCOL: &str = "zuse.account-events.v1";
/// Global outstanding tickets and live sockets on this process.
pub(crate) const GLOBAL_CAPACITY: usize = 4096;
/// One account cannot consume more than this share of either global pool.
pub(crate) const PER_ACTOR_CAPACITY: usize = 16;
pub struct Hub {
    pub(crate) events: EventStore,
    pub(crate) cursors: Cursors,
    pub(crate) tickets: Tickets,
    pub(crate) capacity: Arc<Semaphore>,
    rooms: Mutex<HashMap<String, Weak<Room>>>,
    pub(crate) devices: DeviceLeases,
    dispatch: Notify,
}
pub(crate) struct Room {
    pub(crate) changed: watch::Sender<i64>,
}
impl Hub {
    pub async fn start(
        pool: sqlx::PgPool,
        key: MasterKey,
        issuer: String,
    ) -> Result<Arc<Self>, ApiError> {
        let events = EventStore::new(pool);
        events.ready().await?;
        let hub = Arc::new(Self {
            events,
            cursors: Cursors::new(key, issuer),
            tickets: Tickets::new(GLOBAL_CAPACITY, PER_ACTOR_CAPACITY),
            capacity: Arc::new(Semaphore::new(GLOBAL_CAPACITY)),
            rooms: Mutex::new(HashMap::new()),
            devices: DeviceLeases::new(PER_ACTOR_CAPACITY),
            dispatch: Notify::new(),
        });
        let signal = match std::env::var_os("ZUSE_CLOUD_EVENT_SIGNAL_DIRECTORY") {
            #[cfg(unix)]
            Some(path) => Some(
                super::hint::Listener::bind(std::path::Path::new(&path))
                    .map_err(|_| ApiError::unavailable("account_event_signal_unavailable"))?,
            ),
            _ => None,
        };
        let weak = Arc::downgrade(&hub);
        tokio::spawn(async move {
            let mut recovery = tokio::time::interval(Duration::from_secs(1));
            recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_prune = std::time::Instant::now();
            loop {
                let Some(hub) = weak.upgrade() else { break };
                tokio::select! {
                    _ = hub.dispatch.notified() => {},
                    _ = recovery.tick() => {},
                    _ = async {
                        #[cfg(unix)]
                        if let Some(signal) = &signal { let _ = signal.receive().await; return; }
                        std::future::pending::<()>().await;
                    } => {},
                }
                // A dispatch failure is uncertain, so wake current subscribers to
                // replay the authoritative log even if the commit reply was lost.
                for _ in 0..16 {
                    match hub.events.dispatch().await {
                        Ok(changes) if changes.is_empty() => break,
                        Ok(changes) => {
                            let mut rooms = hub.rooms.lock().await;
                            rooms.retain(|_, room| room.strong_count() > 0);
                            for (owner, sequence) in changes {
                                if let Some(room) = rooms.get(&owner).and_then(Weak::upgrade) {
                                    room.changed.send_replace(sequence);
                                }
                            }
                        }
                        Err(error) => {
                            tracing::warn!(code = error.code, "Account event dispatch deferred");
                            for room in hub.rooms.lock().await.values().filter_map(Weak::upgrade) {
                                room.changed.send_modify(|n| *n = n.wrapping_add(1));
                            }
                            break;
                        }
                    }
                    tokio::task::yield_now().await;
                }
                if last_prune.elapsed() >= Duration::from_secs(3600) {
                    let _ = hub
                        .events
                        .prune(chrono::Utc::now().timestamp_millis() - 7 * 86400_000)
                        .await;
                    last_prune = std::time::Instant::now();
                }
                // Do not retain the authority across an idle wait after shutdown.
                if Arc::strong_count(&hub) == 1 {
                    break;
                }
            }
        });
        Ok(hub)
    }
    pub fn wake(&self) {
        self.dispatch.notify_one();
    }
    pub(crate) async fn room(&self, owner: &str) -> Arc<Room> {
        let mut rooms = self.rooms.lock().await;
        if let Some(room) = rooms.get(owner).and_then(Weak::upgrade) {
            return room;
        }
        rooms.retain(|_, room| room.strong_count() > 0);
        let room = Arc::new(Room {
            changed: watch::channel(0).0,
        });
        rooms.insert(owner.to_owned(), Arc::downgrade(&room));
        room
    }
}

#[derive(Default)]
struct Live {
    /// The current lease for each (actor, device).
    devices: HashMap<(String, uuid::Uuid), (uuid::Uuid, watch::Sender<bool>)>,
    /// Every lease not yet dropped, per actor, including replaced sockets that
    /// are still shutting down and so still hold a shared socket permit.
    outstanding: HashMap<String, usize>,
}
/// Live account-event sockets keyed by actor and device. A device reconnect
/// replaces its previous socket. Each actor's undropped leases are capped, so
/// rapid same-device reconnects cannot pile up sockets in the shared pool.
pub(crate) struct DeviceLeases {
    live: Arc<std::sync::Mutex<Live>>,
    per_actor: usize,
}
/// Releases its slot when dropped, including when an upgrade never completes.
pub(crate) struct DeviceLease {
    live: Arc<std::sync::Mutex<Live>>,
    key: (String, uuid::Uuid),
    id: uuid::Uuid,
    pub(crate) replaced: watch::Receiver<bool>,
}
impl DeviceLeases {
    pub(crate) fn new(per_actor: usize) -> Self {
        Self {
            live: Arc::new(std::sync::Mutex::new(Live::default())),
            per_actor,
        }
    }
    pub(crate) fn claim(&self, actor: &str, device: uuid::Uuid) -> Result<DeviceLease, ApiError> {
        let key = (actor.to_owned(), device);
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let outstanding = live.outstanding.get(actor).copied().unwrap_or(0);
        if outstanding >= self.per_actor {
            // Ask this device's previous socket to leave so a retry can succeed
            // once it has released its permit; never admit beyond the share.
            if let Some((_, old)) = live.devices.get(&key) {
                old.send_replace(true);
            }
            return Err(ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "account_event_actor_capacity",
            ));
        }
        let id = uuid::Uuid::new_v4();
        let (sender, replaced) = watch::channel(false);
        if let Some((_, old)) = live.devices.insert(key.clone(), (id, sender)) {
            old.send_replace(true);
        }
        *live.outstanding.entry(actor.to_owned()).or_default() += 1;
        Ok(DeviceLease {
            live: self.live.clone(),
            key,
            id,
            replaced,
        })
    }
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.live.lock().unwrap().devices.len()
    }
    #[cfg(test)]
    pub(crate) fn outstanding(&self, actor: &str) -> usize {
        let live = self.live.lock().unwrap();
        live.outstanding.get(actor).copied().unwrap_or(0)
    }
}
impl Drop for DeviceLease {
    fn drop(&mut self) {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if live
            .devices
            .get(&self.key)
            .is_some_and(|(id, _)| *id == self.id)
        {
            live.devices.remove(&self.key);
        }
        if let Some(count) = live.outstanding.get_mut(&self.key.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                live.outstanding.remove(&self.key.0);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_leases_cap_distinct_devices_per_actor_and_release_on_drop() {
        let leases = DeviceLeases::new(2);
        let (a, b, c) = (
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
        );
        let first = leases.claim("actor", a).unwrap();
        let second = leases.claim("actor", b).unwrap();
        let error = leases.claim("actor", c).err().unwrap();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        // Another account is unaffected by this actor's usage.
        let other = leases.claim("other", c).unwrap();
        drop(second);
        let third = leases.claim("actor", c).unwrap();
        drop((first, third, other));
        assert_eq!(leases.live(), 0);
        assert_eq!(leases.outstanding("actor"), 0);
    }
    #[test]
    fn replaced_sockets_count_against_the_actor_until_they_drop() {
        let leases = DeviceLeases::new(2);
        let device = uuid::Uuid::new_v4();
        let first = leases.claim("actor", device).unwrap();
        // A reconnect replaces the previous socket, which is told to leave.
        let second = leases.claim("actor", device).unwrap();
        assert!(*first.replaced.borrow());
        assert_eq!(leases.outstanding("actor"), 2);
        // The replaced socket still holds its permit, so a third rapid
        // reconnect is refused rather than exceeding the actor's share.
        let error = leases.claim("actor", device).err().unwrap();
        assert_eq!(error.code, "account_event_actor_capacity");
        assert!(
            *second.replaced.borrow(),
            "the current socket is asked to leave so a retry can succeed"
        );
        drop(first);
        assert_eq!(
            leases.live(),
            1,
            "a replaced lease must not release its successor"
        );
        let third = leases.claim("actor", device).unwrap();
        drop((second, third));
        assert_eq!(leases.outstanding("actor"), 0);
        assert_eq!(leases.live(), 0);
    }
}
