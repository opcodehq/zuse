use crate::{config::Config, error::ApiError};
use async_trait::async_trait;
use futures::StreamExt;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub account_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Membership {
    pub id: String,
    pub user_id: String,
    pub organization_id: String,
    pub status: String,
    pub role: MembershipRole,
}
#[derive(Clone, Debug, Deserialize)]
pub struct MembershipRole {
    pub slug: String,
}

#[async_trait]
pub trait Identity: Send + Sync {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError>;
    async fn membership(&self, account: &str, organization: &str) -> Result<Membership, ApiError>;
    async fn organization_name(&self, organization: &str) -> Result<String, ApiError> {
        Ok(organization.to_owned())
    }
}

struct KeyCache {
    keys: JwkSet,
    fetched: Instant,
}
pub struct WorkosIdentity {
    http: reqwest::Client,
    issuer: String,
    client_id: String,
    jwks_url: reqwest::Url,
    api_key: Option<String>,
    keys: Mutex<Option<KeyCache>>,
}

impl WorkosIdentity {
    pub fn new(config: &Config) -> anyhow::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            issuer: config.workos_issuer.clone(),
            client_id: config.workos_client_id.clone(),
            jwks_url: config.workos_jwks_url.clone(),
            api_key: config.workos_api_key.clone(),
            keys: Mutex::new(None),
        }))
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        let mut cached = self.keys.lock().await;
        let refresh = match cached.as_ref() {
            None => true,
            Some(cache) => {
                cache.fetched.elapsed() >= Duration::from_secs(300)
                    || (cache.keys.find(kid).is_none()
                        && cache.fetched.elapsed() >= Duration::from_secs(30))
            }
        };
        if refresh {
            let response = self
                .http
                .get(self.jwks_url.clone())
                .send()
                .await
                .map_err(|_| ApiError::unavailable("workos_keys_unavailable"))?;
            if !response.status().is_success() {
                return Err(ApiError::unavailable("workos_keys_unavailable"));
            }
            let bytes = bounded_body(response).await?;
            let keys: JwkSet = serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::unavailable("workos_keys_invalid"))?;
            *cached = Some(KeyCache {
                keys,
                fetched: Instant::now(),
            });
        }
        let jwk = cached
            .as_ref()
            .and_then(|c| c.keys.find(kid))
            .ok_or_else(|| ApiError::unauthorized("invalid_workos_token"))?;
        DecodingKey::from_jwk(jwk).map_err(|_| ApiError::unauthorized("invalid_workos_token"))
    }
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    client_id: String,
}

/// WorkOS permits its base issuer, a trailing slash, and user-management issuers.
fn issuer_matches(issuer: &str, configured: &str) -> bool {
    let base = configured.trim_end_matches('/');
    issuer == base
        || issuer == format!("{base}/")
        || issuer.starts_with(&format!("{base}/user_management/"))
}

#[async_trait]
impl Identity for WorkosIdentity {
    async fn verify(&self, token: &str) -> Result<Principal, ApiError> {
        if token.len() > 16384 {
            return Err(ApiError::unauthorized("invalid_workos_token"));
        }
        let invalid = || ApiError::unauthorized("invalid_workos_token");
        let header = decode_header(token).map_err(|_| invalid())?;
        if header.alg != Algorithm::RS256 {
            return Err(invalid());
        }
        let kid = header
            .kid
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or_else(invalid)?;
        let key = self.key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iss", "sub"]);
        // WorkOS binds these access tokens with client_id rather than aud.
        validation.validate_aud = false;
        validation.validate_nbf = true;
        validation.leeway = 0;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| invalid())?
            .claims;
        if !issuer_matches(&claims.iss, &self.issuer)
            || claims.client_id != self.client_id
            || claims.sub.is_empty()
            || claims.sub.len() > 128
            || claims.sub.starts_with("organization:")
        {
            return Err(invalid());
        }
        Ok(Principal {
            account_id: claims.sub,
        })
    }

    async fn organization_name(&self, organization: &str) -> Result<String, ApiError> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("workos_auth_not_configured"))?;
        let mut url = reqwest::Url::parse("https://api.workos.com/organizations/")
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?
            .pop_if_empty()
            .push(organization);
        let response = self
            .http
            .get(url)
            .bearer_auth(key)
            .send()
            .await
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?;
        if !response.status().is_success() {
            return Err(ApiError::unavailable("workos_auth_unavailable"));
        }
        let value: serde_json::Value = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| ApiError::unavailable("workos_auth_invalid_response"))?;
        if value["id"] != organization {
            return Err(ApiError::unavailable("workos_auth_invalid_response"));
        }
        value["name"]
            .as_str()
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unavailable("workos_auth_invalid_response"))
    }
    async fn membership(&self, account: &str, organization: &str) -> Result<Membership, ApiError> {
        let key = self
            .api_key
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("workos_auth_not_configured"))?;
        let response = self
            .http
            .get("https://api.workos.com/user_management/organization_memberships")
            .query(&[
                ("user_id", account),
                ("organization_id", organization),
                ("limit", "100"),
            ])
            .bearer_auth(key)
            .send()
            .await
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?;
        if !response.status().is_success() {
            return Err(ApiError::unavailable("workos_auth_unavailable"));
        }
        #[derive(Deserialize)]
        struct Members {
            data: Vec<Membership>,
        }
        let body: Members = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| ApiError::unavailable("workos_auth_invalid_response"))?;
        body.data
            .into_iter()
            .find(|m| {
                m.user_id == account && m.organization_id == organization && m.status == "active"
            })
            .ok_or_else(|| ApiError::forbidden("organization_access_denied"))
    }
}

pub(crate) async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?;
        if body.len() + chunk.len() > 256 * 1024 {
            return Err(ApiError::unavailable("workos_auth_invalid_response"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts};

    #[tokio::test]
    async fn verifies_signed_workos_identity_and_rejects_wrong_client_issuer_and_expiry() {
        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let der = private.to_pkcs1_der().unwrap();
        let encoding = jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes());
        let public = private.to_public_key();
        let keys = serde_json::json!({"keys":[{"kty":"RSA","kid":"fixture-key","alg":"RS256","use":"sig",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = requests.clone();
        let app = axum::Router::new().route(
            "/jwks",
            axum::routing::get(move || {
                let keys = keys.clone();
                counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async move { axum::Json(keys) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let identity = WorkosIdentity {
            http: reqwest::Client::new(),
            issuer: "https://api.workos.com".into(),
            client_id: "client_fixture".into(),
            jwks_url: format!("http://{address}/jwks").parse().unwrap(),
            api_key: None,
            keys: Mutex::new(None),
        };
        let now = chrono::Utc::now().timestamp();
        let valid = serde_json::json!({"sub":"user_existing","iss":"https://api.workos.com/user_management/client_fixture",
            "client_id":"client_fixture","exp":now+3600});
        let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
        header.kid = Some("fixture-key".into());
        let token = jsonwebtoken::encode(&header, &valid, &encoding).unwrap();
        assert_eq!(
            identity.verify(&token).await.unwrap().account_id,
            "user_existing"
        );
        for (claim, replacement) in [
            ("client_id", serde_json::json!("other-client")),
            ("iss", serde_json::json!("https://api.workos.com.evil.test")),
            ("exp", serde_json::json!(now - 3600)),
            ("sub", serde_json::json!("organization:other")),
        ] {
            let mut invalid = valid.clone();
            invalid[claim] = replacement;
            let token = jsonwebtoken::encode(&header, &invalid, &encoding).unwrap();
            assert_eq!(
                identity.verify(&token).await.unwrap_err().status,
                axum::http::StatusCode::UNAUTHORIZED
            );
        }
        let hs = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(Algorithm::HS256),
            &valid,
            &jsonwebtoken::EncodingKey::from_secret(b"fixture-only"),
        )
        .unwrap();
        assert!(identity.verify(&hs).await.is_err());
        assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 1);
        server.abort();
    }

    #[test]
    fn issuer_is_bound_to_configured_workos_origin() {
        for issuer in [
            "https://api.workos.com",
            "https://api.workos.com/",
            "https://api.workos.com/user_management/client_x",
        ] {
            assert!(issuer_matches(issuer, "https://api.workos.com"));
        }
        for issuer in [
            "https://api.workos.com.evil.test",
            "https://api.workos.com/user_management",
            "https://evil.test",
        ] {
            assert!(!issuer_matches(issuer, "https://api.workos.com"));
        }
    }
}
