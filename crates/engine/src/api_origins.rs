//! General account services and cloud tasks share an origin unless explicitly
//! configured separately. Workspace/relay authentication keeps its own edge URL.
pub(crate) const DEFAULT_API_URL: &str = "https://api.zuse.sh";

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

fn resolve(general: Option<String>, cloud: Option<String>) -> ApiOrigins {
    let general = general
        .or_else(|| cloud.clone())
        .unwrap_or_else(|| DEFAULT_API_URL.into());
    ApiOrigins {
        cloud: cloud.unwrap_or_else(|| general.clone()),
        general,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_keeps_the_existing_production_origin() {
        let value = resolve(None, None);
        assert_eq!(value.general, DEFAULT_API_URL);
        assert_eq!(value.cloud, DEFAULT_API_URL);
    }
    #[test]
    fn cloud_only_staging_override_also_scopes_account_models_and_managed_access() {
        let value = resolve(None, Some("https://stage.example".into()));
        assert_eq!(value.general, "https://stage.example");
        assert_eq!(value.cloud, "https://stage.example");
    }
    #[test]
    fn general_only_override_also_scopes_cloud_tasks() {
        let value = resolve(Some("http://127.0.0.1:27655".into()), None);
        assert_eq!(value.general, "http://127.0.0.1:27655");
        assert_eq!(value.cloud, "http://127.0.0.1:27655");
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
