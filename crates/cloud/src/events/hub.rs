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
pub struct Hub {
    pub(crate) events: EventStore,
    pub(crate) cursors: Cursors,
    pub(crate) tickets: Tickets,
    pub(crate) capacity: Arc<Semaphore>,
    rooms: Mutex<HashMap<String, Weak<Room>>>,
    devices: Mutex<HashMap<(String, uuid::Uuid), (uuid::Uuid, watch::Sender<bool>)>>,
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
            tickets: Tickets::new(4096),
            capacity: Arc::new(Semaphore::new(4096)),
            rooms: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
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
    pub(crate) async fn claim_device(
        &self,
        actor: &str,
        device: uuid::Uuid,
    ) -> (uuid::Uuid, watch::Receiver<bool>) {
        let id = uuid::Uuid::new_v4();
        let (sender, receiver) = watch::channel(false);
        if let Some((_, old)) = self
            .devices
            .lock()
            .await
            .insert((actor.to_owned(), device), (id, sender))
        {
            old.send_replace(true);
        }
        (id, receiver)
    }
    pub(crate) async fn release_device(&self, actor: &str, device: uuid::Uuid, id: uuid::Uuid) {
        let mut devices = self.devices.lock().await;
        if devices
            .get(&(actor.to_owned(), device))
            .is_some_and(|(current, _)| *current == id)
        {
            devices.remove(&(actor.to_owned(), device));
        }
    }
}
