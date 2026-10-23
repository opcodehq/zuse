//! Guest identity, enrollment and storage admission. No control-plane database access.
pub mod authority;
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

pub mod supervisor;

pub mod mailbox;

#[cfg(feature = "engine")]
pub mod engine;

pub mod repository;

pub mod grants;
