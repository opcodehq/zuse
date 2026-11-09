//! OS network-path monitoring → [`crate::wake::set_path_online`].
//!
//! macOS: `NWPathMonitor` (Network.framework, plain C surface + one ObjC
//! block). The instant the OS says the path is back, every parked reconnect
//! backoff redials; while the OS says there is no path, backoff waiters park
//! on the event buses instead of burning dial attempts (see `wake.rs`).
//!
//! Every platform also watches its interface addresses, which is what reveals
//! a move to another network. Platforms without `NWPathMonitor` take their
//! online/offline signal from the same watch.

/// Start the process-wide path monitor. Idempotent; call from engine startup.
pub fn spawn_path_monitor() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        #[cfg(target_os = "macos")]
        imp::start();
        watch_addresses();
    });
}

const ADDRESS_POLL: std::time::Duration = std::time::Duration::from_secs(3);

fn local_addresses() -> Vec<std::net::IpAddr> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| interface.ip())
        .collect();
    addresses.sort();
    addresses
}

/// An opaque fingerprint of the current network, for remembering which route
/// won here. It is a hash so saved settings never contain local addresses.
pub fn network_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    local_addresses().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn watch_addresses() {
    let spawned = std::thread::Builder::new()
        .name("zuse-net-watch".into())
        .spawn(|| {
            let mut previous = local_addresses();
            loop {
                std::thread::sleep(ADDRESS_POLL);
                let current = local_addresses();
                if current == previous {
                    continue;
                }
                #[cfg(not(target_os = "macos"))]
                crate::wake::set_path_online(!current.is_empty());
                previous = current;
                crate::wake::notify_online();
                crate::wake::notify_network_changed();
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "net_path: address watcher unavailable");
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void};

    use block2::{Block, RcBlock};

    // nw_path_monitor_t / nw_path_t / dispatch_queue_t are ObjC objects;
    // opaque pointers are all this bridge needs.
    #[link(name = "Network", kind = "framework")]
    unsafe extern "C" {
        fn nw_path_monitor_create() -> *mut c_void;
        fn nw_path_monitor_set_update_handler(
            monitor: *mut c_void,
            handler: &Block<dyn Fn(*mut c_void)>,
        );
        fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
        fn nw_path_monitor_start(monitor: *mut c_void);
        fn nw_path_get_status(path: *mut c_void) -> i32;
    }

    unsafe extern "C" {
        fn dispatch_queue_create(label: *const c_char, attr: *mut c_void) -> *mut c_void;
    }

    /// `nw_path_status_unsatisfied`. Only a definitive "no path" parks the
    /// dialers — `invalid`/`satisfiable` stay optimistic, so a confused
    /// monitor can only ever make us dial too much, never go silent.
    const NW_PATH_STATUS_UNSATISFIED: i32 = 2;

    pub(super) fn start() {
        unsafe {
            let monitor = nw_path_monitor_create();
            if monitor.is_null() {
                tracing::warn!("net_path: NWPathMonitor unavailable");
                return;
            }
            let handler = RcBlock::new(|path: *mut c_void| {
                let status = nw_path_get_status(path);
                let online = status != NW_PATH_STATUS_UNSATISFIED;
                crate::wake::set_path_online(online);
                if online {
                    // Interface handovers (wifi→hotspot, VPN up/down) arrive
                    // as satisfied→satisfied updates. Old sockets are dead on
                    // the new path either way, so kick parked dials on every
                    // viable-path report — waiters drain stale events before
                    // arming, so a redundant kick costs nothing.
                    crate::wake::notify_online();
                }
            });
            let queue = dispatch_queue_create(c"zuse.net-path".as_ptr(), std::ptr::null_mut());
            nw_path_monitor_set_queue(monitor, queue);
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_start(monitor);
            // The monitor (and its handler copy) lives for the process.
            std::mem::forget(handler);
        }
    }
}
