//! General account services (sign-in, account, billing, models) and cloud tasks
//! use separate default origins: accounts stay on the existing production API,
//! cloud computers and hosted plugins use the Rust cloud backend. One explicit
//! override still scopes both, so staging and local development keep a single
//! origin.
//! Workspace/relay authentication keeps its own edge URL.
pub(crate) const DEFAULT_API_URL: &str = "https://api.zuse.sh";
pub(crate) const DEFAULT_CLOUD_API_URL: &str = "https://api-rust.zuse.sh";

pub(crate) struct ApiOrigins {
    pub general: String,
    pub cloud: String,
}

pub(crate) fn from_env() -> ApiOrigins {
    resolve(
        std::env::var("ZUSE_API_URL").ok(),
        std::env::var("ZUSE_CLOUD_API_URL").ok(),
    )
}

pub(crate) fn resolve(general: Option<String>, cloud: Option<String>) -> ApiOrigins {
    let explicit_general = general.clone();
    let general = general
        .or_else(|| cloud.clone())
        .unwrap_or_else(|| DEFAULT_API_URL.into());
    ApiOrigins {
        cloud: cloud
            .or(explicit_general)
            .unwrap_or_else(|| DEFAULT_CLOUD_API_URL.into()),
        general,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_keeps_accounts_on_the_existing_api_and_cloud_on_rust() {
        let value = resolve(None, None);
        assert_eq!(value.general, "https://api.zuse.sh");
        assert_eq!(value.cloud, "https://api-rust.zuse.sh");
    }
    #[test]
    fn a_general_override_still_scopes_cloud_too() {
        let value = resolve(Some("https://dev.example".into()), None);
        assert_eq!(value.general, "https://dev.example");
        assert_eq!(value.cloud, "https://dev.example");
    }
    #[test]
    fn cloud_only_staging_override_also_scopes_account_models_and_managed_access() {
        let value = resolve(None, Some("https://stage.example".into()));
        assert_eq!(value.general, "https://stage.example");
        assert_eq!(value.cloud, "https://stage.example");
    }
    #[test]
    fn separate_explicit_origins_are_preserved() {
        let value = resolve(
            Some("https://account.example".into()),
            Some("https://tasks.example".into()),
        );
        assert_eq!(value.general, "https://account.example");
        assert_eq!(value.cloud, "https://tasks.example");
    }
    #[test]
    fn explicit_empty_origin_is_not_silently_replaced() {
        let value = resolve(Some(String::new()), Some("https://tasks.example".into()));
        assert_eq!(value.general, "");
        assert_eq!(value.cloud, "https://tasks.example");
    }
}
