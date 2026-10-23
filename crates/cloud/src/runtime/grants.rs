//! Keep the original account authority as the sole refresh writer until explicit migration.
use super::*;
use async_trait::async_trait;
use zuse_protocol::cloud_auth::{CloudAuthProvider, ProviderGrantRequest, SealedProviderGrant};
#[async_trait]
pub trait GrantIssuer: Send + Sync {
    async fn issue(
        &self,
        workspace: &str,
        provider: CloudAuthProvider,
        credential: &str,
        request: &ProviderGrantRequest,
    ) -> Result<SealedProviderGrant, ApiError>;
}
pub struct LegacyGrantIssuer {
    origin: reqwest::Url,
    http: reqwest::Client,
}
impl LegacyGrantIssuer {
    pub fn new(origin: &str, rust_origin: &str) -> Result<Self, ApiError> {
        let origin = reqwest::Url::parse(origin)
            .map_err(|_| ApiError::bad_request("invalid_auth_authority_origin"))?;
        if origin.scheme() != "https"
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || origin.origin().ascii_serialization() == rust_origin
        {
            return Err(ApiError::bad_request("invalid_auth_authority_origin"));
        }
        Ok(Self {
            origin,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| ApiError::unavailable("cloud_auth_transport_unavailable"))?,
        })
    }
}
#[async_trait]
impl GrantIssuer for LegacyGrantIssuer {
    async fn issue(
        &self,
        workspace: &str,
        provider: CloudAuthProvider,
        credential: &str,
        request: &ProviderGrantRequest,
    ) -> Result<SealedProviderGrant, ApiError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| ApiError::bad_request("invalid_auth_authority_origin"))?
            .clear()
            .extend([
                "v1",
                "cloud",
                "workspaces",
                workspace,
                "runtime",
                "providers",
                provider.id(),
                "grant",
            ]);
        let mut response = self
            .http
            .post(url)
            .bearer_auth(credential)
            .json(request)
            .send()
            .await
            .map_err(|_| ApiError::unavailable(provider.reconnecting()))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ApiError::unavailable(provider.reconnecting()))?
        {
            if bytes.len() + chunk.len() > 64 * 1024 {
                return Err(ApiError::unavailable("cloud_provider_grant_invalid"));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            return Err(if body["error"] == provider.reconnect_required() {
                ApiError::unavailable(provider.reconnect_required())
            } else {
                ApiError::unavailable(provider.reconnecting())
            });
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::unavailable("cloud_provider_grant_invalid"))
    }
}
impl RuntimeControl {
    pub fn with_grant_issuer(mut self, issuer: Arc<dyn GrantIssuer>) -> Self {
        self.grant_issuer = Some(issuer);
        self
    }
    async fn grant_binding(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: &ProviderGrantRequest,
        now: i64,
    ) -> Result<(String, String, i64), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if blocked(&row)
            || row["desired_state"] != "ready"
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|hash_value| !equal(hash_value, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
            || generation != request.runtime_generation
            || !enrollment_matches(&row)
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if !storage_enrolled(&row) {
            return Err(ApiError::conflict("runtime_storage_enrollment_required"));
        }
        let config = &row["request_config"];
        if config["providerAuthMode"] != "broker-v1"
            && !(provider == CloudAuthProvider::Codex && config["codexAuthMode"] == "broker-v1")
        {
            return Err(ApiError::conflict("provider-auth-legacy-workspace"));
        }
        let thumb = credential_thumbprint(&request.credential_public_jwk)
            .map_err(|_| ApiError::bad_request("invalid_workspace_key"))?;
        if config["runtimeCredentialKeyThumbprint"] != thumb {
            return Err(ApiError::unauthorized(
                "runtime_credential_key_binding_mismatch",
            ));
        }
        let account = required(&row, "account_id")?;
        if account.starts_with("organization:") {
            return Err(ApiError::forbidden(
                "runtime_organization_access_unavailable",
            ));
        }
        let owner:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='auth-authority' AND resource_id=$1 FOR SHARE").bind(account).fetch_optional(&mut *tx).await?;
        if owner.as_deref() == Some("rust") {
            return Err(ApiError::conflict(
                "cloud_auth_authority_migration_required",
            ));
        }
        let authority: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM api_cloud_auth_authorities a WHERE account_id=$1 FOR SHARE",
        )
        .bind(account)
        .fetch_optional(&mut *tx)
        .await?;
        let authority = authority
            .filter(|row| row["state"] == "ready")
            .ok_or_else(|| ApiError::unavailable(provider.reconnect_required()))?;
        let incarnation = required(&authority, "storage_incarnation_id")?.to_owned();
        let epoch = authority["auth_epoch"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(|| ApiError::unavailable("cloud_auth_authority_invalid"))?;
        tx.commit().await?;
        Ok((thumb, incarnation, epoch))
    }
    pub async fn provider_grant(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: ProviderGrantRequest,
        now: i64,
    ) -> Result<SealedProviderGrant, ApiError> {
        if request.protocol_version != 1
            || uuid::Uuid::parse_str(&request.request_id)
                .ok()
                .is_none_or(|id| {
                    !(1..=8).contains(&id.get_version_num())
                        || id.get_variant() != uuid::Variant::RFC4122
                })
            || request.credential_public_jwk.len() > 16 * 1024
            || request
                .previous_provider_account_id
                .as_ref()
                .is_some_and(|s| s.len() > 256)
            || request
                .previous_chatgpt_account_id
                .as_ref()
                .is_some_and(|s| s.len() > 256)
        {
            return Err(ApiError::bad_request("invalid_provider_grant_request"));
        }
        let issuer = self
            .grant_issuer
            .as_ref()
            .ok_or_else(|| ApiError::unavailable(provider.reconnecting()))?;
        let before = self
            .grant_binding(id, credential, provider, &request, now)
            .await?;
        let grant = issuer.issue(id, provider, credential, &request).await?;
        // Never return a grant completed after runtime revocation, authority rotation or writer handoff.
        let after = self
            .grant_binding(
                id,
                credential,
                provider,
                &request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        if before != after
            || grant.protocol_version != 1
            || grant.request_id != request.request_id
            || grant.key_thumbprint != after.0
            || grant.authority_incarnation_id != after.1
            || grant.authority_epoch != after.2
            || (grant.provider_id != Some(provider)
                && !(provider == CloudAuthProvider::Codex && grant.provider_id.is_none()))
            || grant.wrapped_key.len() > 8192
            || grant.iv.len() > 128
            || grant.ciphertext.len() > 32768
            || grant.tag.len() > 128
            || grant.ciphertext.is_empty()
        {
            return Err(ApiError::conflict("cloud_provider_grant_fence_mismatch"));
        }
        Ok(grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Clone)]
    struct Fixture {
        mode: Arc<AtomicUsize>,
        leaks: Arc<AtomicUsize>,
    }
    async fn grant(
        State(state): State<Fixture>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> Response {
        assert_eq!(headers["authorization"], "Bearer workspace-only-fixture");
        assert_eq!(request["runtimeGeneration"], 2);
        assert_eq!(request["credentialPublicJwk"], "public-only-fixture");
        if state.mode.load(Ordering::SeqCst) == 1 {
            return (StatusCode::TEMPORARY_REDIRECT, [("location", "/leak")]).into_response();
        }
        if state.mode.load(Ordering::SeqCst) == 2 {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"private-upstream-detail"})),
            )
                .into_response();
        }
        Json(json!({"protocolVersion":1,"providerId":"claude","requestId":request["requestId"],"keyThumbprint":"thumb","authorityIncarnationId":"authority","authorityEpoch":1,"wrappedKey":"wrapped","iv":"nonce","ciphertext":"encrypted","tag":"tag"})).into_response()
    }
    #[tokio::test]
    async fn legacy_transport_preserves_request_and_never_redirects_bearer_or_exposes_upstream_errors()
     {
        let state = Fixture {
            mode: Arc::new(AtomicUsize::new(0)),
            leaks: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/v1/cloud/workspaces/workspace-fixture/runtime/providers/claude/grant",
                post(grant),
            )
            .route(
                "/leak",
                post(|State(state): State<Fixture>| async move {
                    state.leaks.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Keep the production client/redirect policy; only its origin is loopback in this private unit fixture.
        let mut issuer = LegacyGrantIssuer::new(
            "https://authority.example.invalid",
            "https://api-rust.zuse.sh",
        )
        .unwrap();
        issuer.origin = reqwest::Url::parse(&origin).unwrap();
        let request = ProviderGrantRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            protocol_version: 1,
            runtime_generation: 2,
            credential_public_jwk: "public-only-fixture".into(),
            reason: zuse_protocol::cloud_auth::GrantReason::Initial,
            previous_provider_account_id: None,
            previous_chatgpt_account_id: None,
        };
        let reply = issuer
            .issue(
                "workspace-fixture",
                CloudAuthProvider::Claude,
                "workspace-only-fixture",
                &request,
            )
            .await
            .unwrap();
        assert_eq!(reply.request_id, request.request_id);
        assert_eq!(reply.ciphertext, "encrypted");
        state.mode.store(1, Ordering::SeqCst);
        let error = issuer
            .issue(
                "workspace-fixture",
                CloudAuthProvider::Claude,
                "workspace-only-fixture",
                &request,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "claude-auth-reconnecting");
        assert_eq!(state.leaks.load(Ordering::SeqCst), 0);
        state.mode.store(2, Ordering::SeqCst);
        let error = issuer
            .issue(
                "workspace-fixture",
                CloudAuthProvider::Claude,
                "workspace-only-fixture",
                &request,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "claude-auth-reconnecting");
        server.abort();
    }
}
