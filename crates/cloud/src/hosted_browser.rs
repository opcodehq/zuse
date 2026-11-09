//! Optional cloud-only browser entry on the existing API origin. Disabled unless
//! both deployment-owned origin and static-client directory are configured.
use crate::config::Config;
use anyhow::{Context, Result, ensure};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct Extension {
    router: axum::Router,
    shutdown: CancellationToken,
}
impl Extension {
    pub fn router(&self) -> axum::Router {
        self.router.clone()
    }
}
impl Drop for Extension {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}
pub fn from_env(config: &Config) -> Result<Option<Extension>> {
    let origin = std::env::var("ZUSE_CLOUD_BROWSER_ORIGIN").ok();
    let assets = std::env::var("ZUSE_CLOUD_BROWSER_ASSETS").ok();
    match (origin, assets) {
        (None, None) => Ok(None),
        (Some(origin), Some(assets)) => configured(
            &origin,
            &std::env::var("CLOUD_PUBLIC_API_URL")
                .context("Hosted browser requires explicit CLOUD_PUBLIC_API_URL")?,
            PathBuf::from(assets),
            &config.workos_client_id,
            config.organization_ids.clone(),
        )
        .map(Some),
        _ => anyhow::bail!("Hosted browser origin and assets must be configured together"),
    }
}
fn configured(
    origin: &str,
    public_api: &str,
    assets: PathBuf,
    public_client: &str,
    organizations: Vec<String>,
) -> Result<Extension> {
    let api = reqwest::Url::parse(public_api).context("Invalid hosted public API origin")?;
    ensure!(
        api.scheme() == "https"
            && api.origin().ascii_serialization() == origin
            && matches!(api.path(), "" | "/")
            && api.query().is_none()
            && api.fragment().is_none()
            && api.username().is_empty()
            && api.password().is_none(),
        "Hosted browser and public API must use the same exact HTTPS origin"
    );
    let assets = assets
        .canonicalize()
        .context("Hosted browser assets are unavailable")?;
    ensure!(
        assets.is_dir() && assets.join("index.html").is_file(),
        "Hosted browser requires a compiled client directory"
    );
    let factory = zuse_engine::hosted_cloud::Factory::new(origin, public_client, organizations)?;
    let shutdown = CancellationToken::new();
    let browser = zuse_serve::hosted::HostedBrowser::new(
        origin,
        Arc::new(factory),
        32,
        Duration::from_secs(8 * 3600),
    )?
    .with_shutdown(shutdown.clone());
    let router = browser.router_with_assets(assets)?;
    Ok(Extension { router, shutdown })
}
