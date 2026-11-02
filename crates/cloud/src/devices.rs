//! Explicitly configured native-device authority, separate from catalog read-only pools.
use crate::{device_proof, error::ApiError, store::Store, tickets::TicketSigner};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct Authority {
    link_gate: tokio::sync::Semaphore,
    tunnels: Option<crate::tunnels::Provider>,
    mint_public_key: String,
    signer: TicketSigner,
    store: Store,
}

impl Authority {
    pub async fn link(&self, account: &str, body: &Value) -> Result<Value, ApiError> {
        // Reserve pool capacity for verification/finalization while the advisory
        // lock holds a connection. Waiting link requests must not hold pool slots.
        let _permit = self
            .link_gate
            .acquire()
            .await
            .map_err(|_| ApiError::unavailable("host_link_unavailable"))?;
        let invalid = || ApiError::bad_request("invalid_environment_link");
        let environment = body["environmentId"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 256
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .ok_or_else(invalid)?;
        let id = body["challengeId"]
            .as_str()
            .filter(|s| s.len() <= 128)
            .ok_or_else(invalid)?;
        let proof = body["proof"].as_str().ok_or_else(invalid)?;
        let public = &body["environmentPublicKey"];
        let label = body["label"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
            .ok_or_else(invalid)?;
        let port = body["origin"]["localHttpPort"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|p| *p != 0)
            .ok_or_else(invalid)?;
        if body["managedTunnel"] != true
            || body["providerKind"] != "desktop"
            || body["wireProtocolVersion"] != 1
            || body["origin"]["localHttpHost"] != "127.0.0.1"
        {
            return Err(invalid());
        }
        let provider = self
            .tunnels
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("managed_tunnel_disabled"))?;
        // Serialize provision/finalize across API processes for this environment.
        let mut gate = self.store.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout = '15s'")
            .execute(&mut *gate)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 37011))")
            .bind(environment)
            .execute(&mut *gate)
            .await?;
        let now = chrono::Utc::now().timestamp_millis();
        self.store
            .verify_pending_host_link(
                account,
                id,
                environment,
                public,
                proof,
                &self.signer.issuer(),
                now,
            )
            .await?;
        let allocation = provider.provision(account, environment, port).await?;
        let http = format!("https://{}", allocation.hostname);
        let ws = format!("wss://{}", allocation.hostname);
        let credential = self
            .store
            .finish_native_host_link_with_tunnel(
                account,
                id,
                environment,
                public,
                proof,
                &self.signer.issuer(),
                label.trim(),
                &http,
                &ws,
                chrono::Utc::now().timestamp_millis(),
                Some(&allocation),
            )
            .await?;
        drop(gate);
        Ok(
            json!({"environmentId":environment,"endpoint":{"httpBaseUrl":http,"wsBaseUrl":ws},"apiIssuer":self.signer.issuer(),"environmentCredential":credential,"mintPublicKey":self.mint_public_key,"tunnelHostname":allocation.hostname,"connectorToken":allocation.connector_token.as_str()}),
        )
    }

    pub async fn link_challenge(&self, account: &str) -> Result<Value, ApiError> {
        self.store
            .create_native_link_challenge(
                account,
                &self.signer.issuer(),
                chrono::Utc::now().timestamp_millis(),
            )
            .await
    }
    pub async fn heartbeat(
        &self,
        environment: &str,
        credential: &str,
        body: &Value,
    ) -> Result<Value, ApiError> {
        self.store
            .heartbeat_native_environment(
                environment,
                credential,
                body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        Ok(json!({"machineAction":null}))
    }
    pub async fn connect(
        &self,
        environment: &str,
        token: &str,
        proof: &str,
    ) -> Result<Value, ApiError> {
        let now = chrono::Utc::now().timestamp_millis();
        let (account, key) = self.signer.verify_device_access(token, "connect", now)?;
        // Identifiers are encoded by clients; reject separators before constructing
        // the canonical proof URL so one grant cannot address another route.
        if environment.is_empty()
            || environment.len() > 256
            || !environment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ApiError::bad_request("invalid_environment_id"));
        }
        let target = format!(
            "{}/v1/environments/{environment}/connect",
            self.signer.issuer()
        );
        let proof = device_proof::verify(proof, "POST", &target, Some(token), now)?;
        if proof.thumbprint != key {
            return Err(ApiError::unauthorized("device_key_mismatch"));
        }
        self.store
            .consume_device_proof(&proof.thumbprint, &proof.jti, proof.issued_at, now)
            .await?;
        let endpoint = self
            .store
            .native_connection_target(&account, environment, &key)
            .await?;
        if endpoint["serviceState"] != "healthy"
            || !endpoint["lastSeenAt"].as_i64().is_some_and(|seen| {
                seen > now.saturating_sub(90000) && seen <= now.saturating_add(30000)
            })
        {
            return Err(ApiError::conflict("environment_offline"));
        }
        if endpoint["wireProtocolVersion"] != 1
            || !endpoint["capabilities"]["features"]
                .as_array()
                .is_some_and(|features| features.iter().any(|f| f == "zuse-rpc-json-v1"))
        {
            return Err(ApiError::conflict("managed_protocol_not_supported"));
        }
        let ws = reqwest::Url::parse(endpoint["wsBaseUrl"].as_str().unwrap_or_default())
            .map_err(|_| ApiError::unavailable("invalid_host_endpoint"))?;
        if ws.scheme() != "wss"
            || !ws.username().is_empty()
            || ws.password().is_some()
            || ws.query().is_some()
            || ws.fragment().is_some()
        {
            return Err(ApiError::unavailable("invalid_host_endpoint"));
        }
        let http = reqwest::Url::parse(endpoint["httpBaseUrl"].as_str().unwrap_or_default())
            .map_err(|_| ApiError::unavailable("invalid_host_endpoint"))?;
        if ws.path() != "/"
            || http.path() != "/"
            || http.scheme() != "https"
            || http.host_str() != ws.host_str()
            || http.port_or_known_default() != ws.port_or_known_default()
            || !http.username().is_empty()
            || http.password().is_some()
            || http.query().is_some()
            || http.fragment().is_some()
        {
            return Err(ApiError::unavailable("invalid_host_endpoint"));
        }
        let token = self
            .signer
            .device_connection_token(&account, environment, &key, now)?;
        Ok(
            json!({"environmentId":environment,"endpoint":{"httpBaseUrl":endpoint["httpBaseUrl"],"wsBaseUrl":endpoint["wsBaseUrl"]},"connectToken":token,"expiresAt":(now/1000+60)*1000}),
        )
    }

    pub fn new(signer: TicketSigner, pool: sqlx::PgPool) -> Self {
        Self {
            link_gate: tokio::sync::Semaphore::new(1),
            tunnels: None,
            mint_public_key: String::new(),
            signer,
            store: Store::from_pool(pool),
        }
    }
    pub async fn register(
        &self,
        token: &str,
        proof: &str,
        body: &Value,
    ) -> Result<Value, ApiError> {
        let now = chrono::Utc::now().timestamp_millis();
        let (account, expected_key) = self.signer.verify_device_access(token, "register", now)?;
        let target = format!("{}/v1/mobile/devices", self.signer.issuer());
        let proof = device_proof::verify(proof, "POST", &target, Some(token), now)?;
        if proof.thumbprint != expected_key {
            return Err(ApiError::unauthorized("device_key_mismatch"));
        }
        let id = body["deviceId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256)
            .ok_or_else(|| ApiError::bad_request("invalid_device"))?;
        let platform = body["platform"]
            .as_str()
            .filter(|p| matches!(*p, "android" | "ios" | "desktop"))
            .ok_or_else(|| ApiError::bad_request("invalid_device"))?;
        if body
            .get("dpopJwk")
            .is_some_and(|key| key != &proof.public_jwk)
        {
            return Err(ApiError::bad_request("device_key_mismatch"));
        }
        let push = body
            .get("pushToken")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .filter(|s| s.len() <= 4096)
                    .ok_or_else(|| ApiError::bad_request("invalid_push_token"))
            })
            .transpose()?;
        self.store
            .consume_device_proof(&proof.thumbprint, &proof.jti, proof.issued_at, now)
            .await?;
        self.store
            .register_native_device(
                id,
                &account,
                platform,
                push,
                &proof.public_jwk,
                &proof.thumbprint,
                now,
            )
            .await?;
        Ok(json!({"deviceId":id,"registered":true}))
    }

    pub async fn from_env() -> anyhow::Result<Option<Arc<Self>>> {
        let Some(path) = std::env::var_os("ZUSE_CLOUD_DEVICE_SIGNING_KEY_FILE") else {
            return Ok(None);
        };
        let path = std::path::Path::new(&path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                std::fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "Device signing key must be owner-readable only"
            );
        }
        let secret = zeroize::Zeroizing::new(std::fs::read_to_string(path)?);
        let issuer = std::env::var("ZUSE_CLOUD_DEVICE_ISSUER")?;
        let signer = TicketSigner::from_jwk(&secret, &issuer)
            .map_err(|_| anyhow::anyhow!("Invalid device signing configuration"))?;
        let url = std::env::var("ZUSE_CLOUD_DEVICE_DATABASE_URL")?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .map_err(|_| anyhow::anyhow!("Cannot connect device authority database"))?;
        let public: Value = serde_json::from_str(&secret)
            .map_err(|_| anyhow::anyhow!("Invalid device signing configuration"))?;
        Ok(Some(Arc::new(Self {
            link_gate: tokio::sync::Semaphore::new(1),
            tunnels: crate::tunnels::Provider::from_env()?,
            mint_public_key: json!({"kty":"OKP","crv":"Ed25519","x":public["x"]}).to_string(),
            signer,
            store: Store::from_pool(pool),
        })))
    }

    pub async fn mint(&self, account: &str, proof: &str) -> Result<Value, ApiError> {
        let now = chrono::Utc::now().timestamp_millis();
        let target = format!("{}/v1/client/dpop-token", self.signer.issuer());
        let proof = device_proof::verify(proof, "POST", &target, None, now)?;
        self.store
            .consume_device_proof(&proof.thumbprint, &proof.jti, proof.issued_at, now)
            .await?;
        let access = self
            .signer
            .device_access_token(account, &proof.thumbprint, now)?;
        Ok(json!({"accessToken": access, "expiresIn":300000}))
    }
}
