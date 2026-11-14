//! Which networked features this build shows.
//!
//! [`enabled`] covers cloud: cloud workspaces, the Zuse account button, sync,
//! account-linked computers and hosted plugins. [`remote_enabled`] covers
//! remote access: pairing other computers, phones and browsers with this one.
//!
//! Release builds are local-only by default. Development builds keep both on
//! so the work in progress stays reachable. `ZUSE_CLOUD` / `ZUSE_REMOTE` set
//! to `1` turn a feature on in a release build, and `0` turns it off in a
//! development build. Browsers have no process environment, so they follow
//! the build default.

use std::sync::OnceLock;

/// Cloud features. Read once per process; a restart picks up a changed
/// `ZUSE_CLOUD`.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| resolve(std::env::var("ZUSE_CLOUD").ok().as_deref()))
}

/// Remote access (pairing and other computers). Read once per process; a
/// restart picks up a changed `ZUSE_REMOTE`.
pub fn remote_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| resolve(std::env::var("ZUSE_REMOTE").ok().as_deref()))
}

/// Placeholder for the archived-chats search, which only covers Cloud
/// archives when cloud is on.
pub fn archive_search_placeholder() -> &'static str {
    if enabled() {
        "Search saved chats or Cloud archives…"
    } else {
        "Search saved chats…"
    }
}

fn resolve(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some("1" | "true" | "on") => true,
        Some("0" | "false" | "off") => false,
        _ => cfg!(debug_assertions),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn env_overrides_the_build_default() {
        assert!(resolve(Some("1")));
        assert!(resolve(Some(" true ")));
        assert!(!resolve(Some("0")));
        assert!(!resolve(Some("off")));
        assert_eq!(resolve(None), cfg!(debug_assertions));
        assert_eq!(resolve(Some("maybe")), cfg!(debug_assertions));
    }
}
