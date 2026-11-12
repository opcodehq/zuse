//! Guest identity, enrollment and storage admission. No control-plane database access.
pub mod authority;
pub mod capabilities;
pub mod identity;
pub mod storage;
pub use authority::Authority;
pub use identity::{Identity, IdentityBinding};
pub use storage::{DataLock, Storage};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
pub mod parking;
pub mod supervisor;

pub mod mailbox;

#[cfg(feature = "engine")]
pub mod engine;
#[cfg(feature = "engine")]
pub mod task_progress;

pub mod preparation;
pub mod repository;
pub mod startup;
pub mod workspace_wait;

pub mod environment;
pub mod tool_output;

pub mod grants;
#[cfg(unix)]
pub mod workload_secrets;

#[cfg(all(unix, feature = "engine"))]
pub mod grok_auth;

#[cfg(feature = "engine")]
pub mod lifecycle;
#[cfg(all(unix, feature = "engine"))]
pub mod memory_transport;

#[cfg(feature = "engine")]
pub mod projection;

#[cfg(feature = "engine")]
pub mod recovery;

pub mod image;

#[cfg(unix)]
pub mod github;

#[cfg(target_os = "linux")]
pub mod native_sockets;

pub mod project_cache;
#[cfg(feature = "engine")]
pub mod plugins;
