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
    /// Human-readable account (name and email) for confirmation pages.
    async fn account_label(&self, _account: &str) -> Result<Option<String>, ApiError> {
        Ok(None)
    }
}

struct KeyCache {
    keys: JwkSet,
    fetched: Instant,
}
/// Key sets older than this are refreshed before signing keys are trusted.
const KEY_TTL: Duration = Duration::from_secs(300);
/// An unknown `kid` triggers at most one refresh attempt per floor interval.
const UNKNOWN_KID_REFETCH_FLOOR: Duration = Duration::from_secs(30);
pub struct WorkosIdentity {
    http: reqwest::Client,
    issuer: String,
    client_id: String,
    jwks_url: reqwest::Url,
    api_key: Option<String>,
    /// Read on every request; never held across an await.
    keys: std::sync::RwLock<Option<KeyCache>>,
    /// Single-flight refresh. Holds the time of the last attempt (success or not).
    refresh: Mutex<Option<Instant>>,
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
            keys: std::sync::RwLock::new(None),
            refresh: Mutex::new(None),
        }))
    }

    fn cached(&self, kid: &str) -> (Option<DecodingKey>, bool) {
        let cache = self.keys.read().unwrap_or_else(|e| e.into_inner());
        let Some(cache) = cache.as_ref() else {
            return (None, false);
        };
        let key = cache
            .keys
            .find(kid)
            .and_then(|jwk| DecodingKey::from_jwk(jwk).ok());
        (key, cache.fetched.elapsed() < KEY_TTL)
    }

    /// Requests whose `kid` is cached never wait on the network while another
    /// request refreshes. Unknown kids share one refresh, at most once per floor.
    async fn key(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        let invalid = || ApiError::unauthorized("invalid_workos_token");
        let (cached, fresh) = self.cached(kid);
        if let (Some(key), true) = (&cached, fresh) {
            return Ok(key.clone());
        }
        let mut last_attempt = match self.refresh.try_lock() {
            Ok(guard) => guard,
            // Another request is already refreshing. A known (stale) key keeps
            // working meanwhile; an unknown kid waits for that single fetch.
            Err(_) => {
                if let Some(key) = cached {
                    return Ok(key);
                }
                let guard = self.refresh.lock().await;
                if let (Some(key), _) = self.cached(kid) {
                    return Ok(key);
                }
                guard
            }
        };
        // Re-check after winning the lock: a refresh may have just completed.
        let (cached, fresh) = self.cached(kid);
        if let (Some(key), true) = (&cached, fresh) {
            return Ok(key.clone());
        }
        let has_cache = self
            .keys
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if cached.is_none()
            && has_cache
            && last_attempt.is_some_and(|at| at.elapsed() < UNKNOWN_KID_REFETCH_FLOOR)
        {
            return Err(invalid());
        }
        *last_attempt = Some(Instant::now());
        let keys = self.fetch_keys().await?;
        let key = keys
            .find(kid)
            .ok_or_else(invalid)
            .and_then(|jwk| DecodingKey::from_jwk(jwk).map_err(|_| invalid()));
        *self.keys.write().unwrap_or_else(|e| e.into_inner()) = Some(KeyCache {
            keys,
            fetched: Instant::now(),
        });
        drop(last_attempt);
        key
    }

    async fn fetch_keys(&self) -> Result<JwkSet, ApiError> {
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
        serde_json::from_slice(&bytes).map_err(|_| ApiError::unavailable("workos_keys_invalid"))
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
    async fn account_label(&self, account: &str) -> Result<Option<String>, ApiError> {
        let Some(key) = self.api_key.as_ref() else {
            return Ok(None);
        };
        let mut url = reqwest::Url::parse("https://api.workos.com/user_management/users/")
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("workos_auth_unavailable"))?
            .pop_if_empty()
            .push(account);
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
        if value["id"] != account {
            return Err(ApiError::unavailable("workos_auth_invalid_response"));
        }
        Ok(account_label(&value))
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

/// "First Last <email>", or whichever part exists.
fn account_label(user: &serde_json::Value) -> Option<String> {
    let text = |key: &str| {
        user[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
    };
    let name = [text("first_name"), text("last_name")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    match (name.is_empty(), text("email")) {
        (false, Some(email)) => Some(format!("{name} <{email}>")),
        (true, Some(email)) => Some(email.to_owned()),
        (false, None) => Some(name),
        (true, None) => None,
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
            keys: std::sync::RwLock::new(None),
            refresh: Mutex::new(None),
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unknown_kid_refresh_is_single_flight_and_never_blocks_known_keys() {
        let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let der = private.to_pkcs1_der().unwrap();
        let encoding = jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes());
        let public = private.to_public_key();
        let keys = serde_json::json!({"keys":[{"kty":"RSA","kid":"fixture-key","alg":"RS256","use":"sig",
            "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (counted, delayed) = (requests.clone(), slow.clone());
        let app = axum::Router::new().route(
            "/jwks",
            axum::routing::get(move || {
                let keys = keys.clone();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let delay = delayed.load(std::sync::atomic::Ordering::SeqCst);
                async move {
                    if delay {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    axum::Json(keys)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let identity = Arc::new(WorkosIdentity {
            http: reqwest::Client::new(),
            issuer: "https://api.workos.com".into(),
            client_id: "client_fixture".into(),
            jwks_url: format!("http://{address}/jwks").parse().unwrap(),
            api_key: None,
            keys: std::sync::RwLock::new(None),
            refresh: Mutex::new(None),
        });
        let claims = serde_json::json!({"sub":"user_existing","iss":"https://api.workos.com",
            "client_id":"client_fixture","exp":chrono::Utc::now().timestamp()+3600});
        let token = |kid: &str| {
            let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
            header.kid = Some(kid.into());
            jsonwebtoken::encode(&header, &claims, &encoding).unwrap()
        };
        let known = token("fixture-key");
        identity.verify(&known).await.unwrap();
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Inside the floor an unknown kid is rejected without any network call.
        assert!(identity.verify(&token("rotated-key")).await.is_err());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Past the floor, with a slow key endpoint, many unknown kids arrive at once.
        *identity.refresh.lock().await = Instant::now().checked_sub(Duration::from_secs(60));
        slow.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut unknown = Vec::new();
        for n in 0..8 {
            let (identity, token) = (identity.clone(), token(&format!("unknown-{n}")));
            unknown.push(tokio::spawn(async move {
                identity.verify(&token).await.is_err()
            }));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        identity.verify(&known).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a cached kid must not wait for the in-flight refresh"
        );
        for task in unknown {
            assert!(task.await.unwrap());
        }
        assert_eq!(
            requests.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "concurrent unknown kids share one refresh"
        );
        server.abort();
    }

    #[test]
    fn account_label_prefers_name_and_email_and_ignores_unsafe_values() {
        let label = |v: serde_json::Value| account_label(&v);
        assert_eq!(
            label(
                serde_json::json!({"first_name":"Ada","last_name":"Lovelace","email":"ada@example.test"})
            ),
            Some("Ada Lovelace <ada@example.test>".into())
        );
        assert_eq!(
            label(serde_json::json!({"email":"ada@example.test","first_name":" "})),
            Some("ada@example.test".into())
        );
        assert_eq!(
            label(serde_json::json!({"first_name":"Ada\u{0}","email":null})),
            None
        );
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
