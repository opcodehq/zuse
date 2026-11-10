use anyhow::{Context, Result, ensure};
use reqwest::Url;
use std::{env, net::SocketAddr};

/// No Debug: database and WorkOS credentials must not enter startup logs.
pub struct Config {
    pub listen: SocketAddr,
    pub database_url: String,
    pub workos_issuer: String,
    pub workos_jwks_url: Url,
    pub workos_client_id: String,
    pub workos_api_key: Option<String>,
    pub organizations_enabled: bool,
    pub organization_ids: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub gateway_max_sockets: usize,
    pub gateway_queued_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url = env::var("ZUSE_CLOUD_DATABASE_URL").context(
            "ZUSE_CLOUD_DATABASE_URL is required; DATABASE_URL is never used implicitly",
        )?;
        let client = env::var("WORKOS_CLIENT_ID")
            .unwrap_or_else(|_| "client_01KWGQ818571ARFATQ3G9AR2Y2".into());
        let jwks = env::var("WORKOS_JWKS_URL")
            .unwrap_or_else(|_| format!("https://api.workos.com/sso/jwks/{client}"));
        let issuer = env::var("WORKOS_ISSUER").unwrap_or_else(|_| "https://api.workos.com".into());
        let jwks = https_url(&jwks)?;
        let issuer = https_url(&issuer)?
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        ensure!(
            jwks.path().ends_with(&format!("/jwks/{client}")),
            "WORKOS_JWKS_URL must match WORKOS_CLIENT_ID"
        );
        let allowed_origins = env::var("ZUSE_CLOUD_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for origin in &allowed_origins {
            let url = https_url(origin)?;
            ensure!(
                url.origin().ascii_serialization() == *origin,
                "CORS entries must be HTTPS origins"
            );
        }
        Ok(Self {
            gateway_max_sockets: env::var("ZUSE_CLOUD_GATEWAY_MAX_SOCKETS")
                .unwrap_or_else(|_| "256".into())
                .parse()
                .context("Invalid ZUSE_CLOUD_GATEWAY_MAX_SOCKETS")?,
            gateway_queued_bytes: env::var("ZUSE_CLOUD_GATEWAY_QUEUED_BYTES")
                .unwrap_or_else(|_| "33554432".into())
                .parse()
                .context("Invalid ZUSE_CLOUD_GATEWAY_QUEUED_BYTES")?,
            listen: env::var("ZUSE_CLOUD_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()?,
            database_url,
            workos_issuer: issuer,
            workos_jwks_url: jwks,
            workos_client_id: client,
            workos_api_key: env::var("WORKOS_API_KEY").ok().filter(|s| !s.is_empty()),
            organizations_enabled: env::var("ORGANIZATION_WORKSPACES_ENABLED")
                .is_ok_and(|v| v == "true"),
            organization_ids: env::var("ZUSE_CLOUD_ORGANIZATION_IDS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            allowed_origins,
        })
    }
}

fn https_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("Invalid service URL")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Service URLs require HTTPS without credentials, query or fragment"
    );
    Ok(url)
}
