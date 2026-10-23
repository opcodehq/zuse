//! Short-lived, repository-scoped GitHub installation grants. Never persisted in build records.
use super::*;
use async_trait::async_trait;
use rsa::{
    pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey},
    pkcs8::DecodePrivateKey,
};
use std::collections::BTreeMap;
use zeroize::Zeroizing;
pub struct Grant {
    pub installation: i64,
    pub token: Zeroizing<String>,
}
#[async_trait]
pub trait GrantIssuer: Send + Sync {
    async fn mint(&self, operation: &Operation) -> Result<Vec<Grant>, ApiError>;
}
pub struct GithubIssuer {
    http: reqwest::Client,
    key: jsonwebtoken::EncodingKey,
    app: String,
    endpoint: String,
}
impl GithubIssuer {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let id = std::env::var("GITHUB_APP_ID").unwrap_or_default();
        let pem = Zeroizing::new(std::env::var("GITHUB_APP_PRIVATE_KEY").unwrap_or_default());
        if id.is_empty() && pem.is_empty() {
            return Ok(None);
        }
        anyhow::ensure!(
            !id.is_empty() && !pem.is_empty(),
            "Image GitHub grants require GITHUB_APP_ID and GITHUB_APP_PRIVATE_KEY"
        );
        let pem = Zeroizing::new(pem.replace("\\n", "\n"));
        let private = rsa::RsaPrivateKey::from_pkcs8_pem(&pem)
            .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_pem(&pem))
            .map_err(|_| anyhow::anyhow!("Invalid GitHub image signing key"))?;
        let der = private
            .to_pkcs1_der()
            .map_err(|_| anyhow::anyhow!("Invalid GitHub image signing key"))?;
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()?,
            key: jsonwebtoken::EncodingKey::from_rsa_der(der.as_bytes()),
            app: id,
            endpoint: "https://api.github.com".into(),
        }))
    }
}
#[async_trait]
impl GrantIssuer for GithubIssuer {
    async fn mint(&self, op: &Operation) -> Result<Vec<Grant>, ApiError> {
        if now() > op.approved_until {
            return Err(ApiError::conflict("image_repository_authorization_expired"));
        }
        let mut installations: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for repository in &op.repositories {
            if let Some(id) = repository.installation {
                let name = text(&repository.row, "repository_identity")?
                    .strip_prefix("github.com/")
                    .and_then(|s| s.split_once('/'))
                    .map(|(_, name)| name)
                    .ok_or_else(unavailable)?;
                installations.entry(id).or_default().push(name.into());
            }
        }
        let mut grants = Vec::new();
        for (installation, repositories) in installations {
            #[derive(Serialize)]
            struct Claims<'a> {
                iat: i64,
                exp: i64,
                iss: &'a str,
            }
            let seconds = now() / 1000;
            let jwt = Zeroizing::new(
                jsonwebtoken::encode(
                    &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
                    &Claims {
                        iat: seconds - 60,
                        exp: seconds + 540,
                        iss: &self.app,
                    },
                    &self.key,
                )
                .map_err(|_| unavailable())?,
            );
            let mut response = self
                .http
                .post(format!(
                    "{}/app/installations/{installation}/access_tokens",
                    self.endpoint
                ))
                .header("user-agent", "zuse-rust-cloud")
                .header("accept", "application/vnd.github+json")
                .header("x-github-api-version", "2022-11-28")
                .bearer_auth(jwt.as_str())
                .json(&json!({"permissions":{"contents":"read"},"repositories":repositories}))
                .send()
                .await
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(ApiError::unavailable("image_github_grant_unavailable"));
            }
            let mut bytes = Zeroizing::new(Vec::new());
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if bytes.len() + chunk.len() > 65536 {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&chunk);
            }
            #[derive(Deserialize)]
            struct Token {
                token: String,
                expires_at: String,
            }
            let token: Token = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
            let secret = Zeroizing::new(token.token);
            if secret.is_empty()
                || secret.contains(['\n', '\r', '\0'])
                || chrono::DateTime::parse_from_rfc3339(&token.expires_at)
                    .map_err(|_| unavailable())?
                    .timestamp_millis()
                    < now() + 60_000
            {
                return Err(unavailable());
            }
            grants.push(Grant {
                installation,
                token: secret,
            });
        }
        Ok(grants)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use rsa::pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn installation_tokens_use_signed_app_identity_and_only_approved_repositories() {
        let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let private = key.to_pkcs1_der().unwrap();
        let public = key.to_public_key().to_pkcs1_der().unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let count = called.clone();
        let fixture=Router::new().route("/app/installations/42/access_tokens",post(move|headers:HeaderMap,Json(body):Json<Value>|{let count=count.clone();let public=public.clone();async move{
            assert_eq!(headers["x-github-api-version"],"2022-11-28");assert_eq!(body,json!({"permissions":{"contents":"read"},"repositories":["selected"]}));
            let token=headers["authorization"].to_str().unwrap().strip_prefix("Bearer ").unwrap();let claims=jsonwebtoken::decode::<Value>(token,&jsonwebtoken::DecodingKey::from_rsa_der(public.as_bytes()),&jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256)).unwrap().claims;
            assert_eq!(claims["iss"],"123");assert!(claims["exp"].as_i64().unwrap()-claims["iat"].as_i64().unwrap()<=600);count.fetch_add(1,Ordering::SeqCst);
            Json(json!({"token":"fixture-short-lived-installation-token","expires_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()}))
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
        let issuer = GithubIssuer {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            key: jsonwebtoken::EncodingKey::from_rsa_der(private.as_bytes()),
            app: "123".into(),
            endpoint,
        };
        let mut op = Operation {
            version: 1,
            build: "test".into(),
            account: "user-alice".into(),
            phase: "prepare".into(),
            config: ImageConfig {
                base_snapshot: "base".into(),
                base_version: 1,
                template_version: "template".into(),
                max_cache_bytes: 1024,
            },
            plan: ImagePlan {
                build_id: "test".into(),
                template_version: "template".into(),
                configuration_digest: "digest".into(),
                home: "/home/zuse".into(),
                cache_root: "/home/repos".into(),
                status_directory: "/var/lib/zuse/build".into(),
                secrets_root: "/run/zuse-secrets".into(),
                max_cache_bytes: 1024,
                repositories: vec![],
            },
            repositories: vec![
                RepositoryPin {
                    row: json!({"repository_identity":"github.com/owner/selected"}),
                    installation: Some(42),
                },
                RepositoryPin {
                    row: json!({"repository_identity":"github.com/owner/public"}),
                    installation: None,
                },
            ],
            approved_until: now() + 60_000,
            machine_name: "builder".into(),
            snapshot_name: "image".into(),
            verify_name: "verify".into(),
            machine: None,
            verifier: None,
            snapshot: None,
            snapshot_version: None,
            manifest: None,
            verified: false,
            failure: None,
            created: now(),
            resources: None,
            verifier_started_at: None,
        };
        let grants = issuer.mint(&op).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].installation, 42);
        assert_eq!(
            grants[0].token.as_str(),
            "fixture-short-lived-installation-token"
        );
        assert!(
            !serde_json::to_string(&op)
                .unwrap()
                .contains(grants[0].token.as_str())
        );
        op.approved_until = now() - 1;
        assert_eq!(
            issuer.mint(&op).await.err().unwrap().code,
            "image_repository_authorization_expired"
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
