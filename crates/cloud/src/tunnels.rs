//! Managed tunnel control-plane client. Connector and API tokens are never logged.
use crate::error::ApiError;
use futures::StreamExt;
use serde_json::Value;
use std::time::Duration;

pub struct Provider {
    #[cfg(test)]
    test_origin: Option<String>,
    http: reqwest::Client,
    token: zeroize::Zeroizing<String>,
    account: String,
    zone: String,
    domain: String,
    namespace: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::Uri};
    use serde_json::json;
    use std::{collections::VecDeque, sync::Arc};
    use tokio::sync::Mutex;
    type Log = Arc<Mutex<(VecDeque<Value>, Vec<(String, Value)>)>>;
    #[tokio::test]
    async fn provider_failures_are_redacted_and_redirects_are_not_followed() {
        use axum::{
            http::{StatusCode, header},
            response::IntoResponse,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let redirect = format!("{origin}/credential-target");
        let app = Router::new().fallback(move |uri: Uri| {
            let hits = counter.clone();
            let redirect = redirect.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                match uri.path() {
                    "/redirect" => {
                        (StatusCode::FOUND, [(header::LOCATION, redirect)]).into_response()
                    }
                    "/envelope" => Json(
                        json!({"success":false,"errors":[{"message":"PRIVATE_PROVIDER_DETAIL"}]}),
                    )
                    .into_response(),
                    "/large" => "x".repeat(2_000_001).into_response(),
                    _ => (StatusCode::FORBIDDEN, "PRIVATE_PROVIDER_DETAIL").into_response(),
                }
            }
        });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut provider = Provider::new(
            "test-secret".into(),
            "b".repeat(32),
            "c".repeat(32),
            "connect.example".into(),
            "staging".into(),
        )
        .unwrap();
        provider.test_origin = Some(origin);
        provider.http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        for path in ["/redirect", "/envelope", "/large", "/forbidden"] {
            let error = provider
                .request(reqwest::Method::GET, path, None)
                .await
                .unwrap_err();
            assert_eq!(error.code, "tunnel_provider_unavailable");
            assert!(!format!("{error:?}").contains("PRIVATE"));
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            4,
            "Redirect must not forward credentials"
        );
        assert!(
            provider
                .provision("user-alice", "desktop", 0)
                .await
                .is_err()
        );
        assert!(
            provider
                .provision("user/alice", "desktop", 28655)
                .await
                .is_err()
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            4,
            "Invalid inputs must fail before provider calls"
        );
        task.abort();
    }
    async fn reply(State(log): State<Log>, uri: Uri, body: axum::body::Bytes) -> Json<Value> {
        let mut log = log.lock().await;
        log.1.push((
            uri.to_string(),
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            },
        ));
        Json(
            json!({"success":true,"result":log.0.pop_front().expect("Unexpected provider request")}),
        )
    }
    #[tokio::test]
    async fn named_tunnel_provisioning_configures_loopback_dns_and_token() {
        let tunnel = "11111111-1111-4111-8111-111111111111";
        let dns = "a".repeat(32);
        let log: Log = Arc::new(Mutex::new((
            VecDeque::from(vec![
                json!([]),
                json!({"id":tunnel}),
                json!({}),
                json!([]),
                json!({"id":dns}),
                json!("test-connector-token"),
            ]),
            vec![],
        )));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(reply).with_state(log.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut provider = Provider::new(
            "test-api-token".into(),
            "b".repeat(32),
            "c".repeat(32),
            "connect.example".into(),
            "staging".into(),
        )
        .unwrap();
        provider.test_origin = Some(origin);
        provider.http = reqwest::Client::new();
        let allocation = provider
            .provision("user-alice", "desktop", 28655)
            .await
            .unwrap();
        assert_eq!(allocation.tunnel_id, tunnel);
        assert_eq!(allocation.dns_record_id, dns);
        assert_eq!(allocation.connector_token.as_str(), "test-connector-token");
        let log = log.lock().await;
        assert_eq!(log.1.len(), 6);
        assert!(log.0.is_empty());
        assert_eq!(log.1[1].1["config_src"], "cloudflare");
        assert_eq!(
            log.1[2].1["config"]["ingress"][0]["service"],
            "http://127.0.0.1:28655"
        );
        assert_eq!(
            log.1[2].1["config"]["ingress"][0]["hostname"],
            allocation.hostname
        );
        assert_eq!(log.1[4].1["content"], format!("{tunnel}.cfargotunnel.com"));
        assert_eq!(log.1[4].1["proxied"], true);
        assert!(log.1[5].0.ends_with("/token"));
        task.abort();
    }
}
pub struct Allocation {
    pub hostname: String,
    pub tunnel_id: String,
    pub dns_record_id: String,
    pub connector_token: zeroize::Zeroizing<String>,
}
impl Provider {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(path) = std::env::var_os("ZUSE_CLOUD_TUNNEL_CONFIG_FILE") else {
            return Ok(None);
        };
        let path = std::path::Path::new(&path);
        let metadata = std::fs::metadata(path)?;
        anyhow::ensure!(metadata.len() <= 16384, "Tunnel configuration is too large");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "Tunnel configuration must be owner-readable only"
            );
        }
        let raw = zeroize::Zeroizing::new(std::fs::read_to_string(path)?);
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Config {
            token: String,
            account_id: String,
            zone_id: String,
            base_domain: String,
            namespace: String,
        }
        let config: Config = serde_json::from_str(&raw)
            .map_err(|_| anyhow::anyhow!("Invalid tunnel configuration"))?;
        Self::new(
            config.token,
            config.account_id,
            config.zone_id,
            config.base_domain,
            config.namespace,
        )
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Invalid tunnel configuration"))
    }

    pub async fn provision(
        &self,
        account: &str,
        environment: &str,
        port: u16,
    ) -> Result<Allocation, ApiError> {
        use reqwest::Method;
        use serde_json::json;
        use sha2::{Digest, Sha256};
        let valid_identity = |s: &str| {
            !s.is_empty()
                && s.len() <= 256
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        };
        if port == 0 || !valid_identity(account) || !valid_identity(environment) {
            return Err(ApiError::bad_request("invalid_tunnel_origin"));
        }
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("{}:{account}:{environment}", self.namespace).as_bytes())
        );
        let name = format!("{}-{}", self.namespace, &digest[..32]);
        let hostname = format!("{name}.{}", self.domain);
        if hostname.len() > 253 {
            return Err(ApiError::bad_request("invalid_tunnel_hostname"));
        }
        let base = format!("/accounts/{}/cfd_tunnel", self.account);
        let search = format!("{base}?name={name}&is_deleted=false");
        let found = self.request(Method::GET, &search, None).await?;
        let existing = found
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["name"] == name))
            .cloned();
        let tunnel = match existing {
            Some(tunnel) => tunnel,
            None => match self
                .request(
                    Method::POST,
                    &base,
                    Some(json!({"name":name,"config_src":"cloudflare"})),
                )
                .await
            {
                Ok(tunnel) => tunnel,
                Err(error) => self
                    .request(Method::GET, &search, None)
                    .await?
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| row["name"] == name))
                    .cloned()
                    .ok_or(error)?,
            },
        };
        let id = tunnel["id"]
            .as_str()
            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            .ok_or_else(|| ApiError::unavailable("invalid_tunnel_response"))?
            .to_owned();
        self.request(Method::PUT,&format!("{base}/{id}/configurations"),Some(json!({"config":{"ingress":[{"hostname":hostname,"service":format!("http://127.0.0.1:{port}")},{"service":"http_status:404"}]}}))).await?;
        let dns = format!("/zones/{}/dns_records", self.zone);
        let records = self
            .request(
                Method::GET,
                &format!("{dns}?type=CNAME&name={hostname}"),
                None,
            )
            .await?;
        let existing = records
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["name"] == hostname))
            .and_then(|row| row["id"].as_str())
            .map(str::to_owned);
        let record = json!({"type":"CNAME","name":hostname,"content":format!("{id}.cfargotunnel.com"),"proxied":true});
        let record = if let Some(record_id) = existing {
            if record_id.len() != 32 || !record_id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(ApiError::unavailable("invalid_dns_response"));
            }
            self.request(Method::PUT, &format!("{dns}/{record_id}"), Some(record))
                .await?
        } else {
            self.request(Method::POST, &dns, Some(record)).await?
        };
        let dns_record_id = record["id"]
            .as_str()
            .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| ApiError::unavailable("invalid_dns_response"))?
            .to_owned();
        let token = self
            .request(Method::GET, &format!("{base}/{id}/token"), None)
            .await?;
        let token = token
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 16384 && !s.chars().any(char::is_control))
            .ok_or_else(|| ApiError::unavailable("invalid_connector_token"))?;
        Ok(Allocation {
            hostname,
            tunnel_id: id,
            dns_record_id,
            connector_token: zeroize::Zeroizing::new(token.into()),
        })
    }

    pub fn new(
        token: String,
        account: String,
        zone: String,
        domain: String,
        namespace: String,
    ) -> Result<Self, ApiError> {
        let valid_id = |s: &str| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let valid_label = |s: &str| {
            !s.is_empty()
                && s.len() <= 63
                && !s.starts_with('-')
                && !s.ends_with('-')
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if token.is_empty()
            || token.len() > 8192
            || !valid_id(&account)
            || !valid_id(&zone)
            || domain.len() > 253
            || !domain.contains('.')
            || !domain.split('.').all(valid_label)
            || namespace.len() > 20
            || !valid_label(&namespace)
        {
            return Err(ApiError::bad_request("invalid_tunnel_configuration"));
        }
        let http = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| ApiError::unavailable("tunnel_client_unavailable"))?;
        Ok(Self {
            #[cfg(test)]
            test_origin: None,
            http,
            token: zeroize::Zeroizing::new(token),
            account,
            zone,
            domain,
            namespace,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        let failed = || ApiError::unavailable("tunnel_provider_unavailable");
        let origin = "https://api.cloudflare.com/client/v4";
        #[cfg(test)]
        let origin = self.test_origin.as_deref().unwrap_or(origin);
        let mut request = self
            .http
            .request(method, format!("{origin}{path}"))
            .bearer_auth(self.token.as_str());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| failed())?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|n| n > 2_000_000)
        {
            return Err(failed());
        }
        let mut data = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| failed())?;
            if data.len() + chunk.len() > 2_000_000 {
                return Err(failed());
            }
            data.extend_from_slice(&chunk);
        }
        let envelope: Value = serde_json::from_slice(&data).map_err(|_| failed())?;
        if envelope["success"] != true {
            return Err(failed());
        }
        envelope.get("result").cloned().ok_or_else(failed)
    }
}
