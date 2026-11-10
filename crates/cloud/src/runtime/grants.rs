//! Keep the original account authority as the sole refresh writer until explicit migration.
use super::*;
use async_trait::async_trait;
use zuse_protocol::cloud_auth::{CloudAuthProvider, ProviderGrantRequest, SealedProviderGrant};
mod agents;
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
// The selected identity is already known under the runtime/workspace fence.
// Carry that snapshot into issuance; the final binding still rechecks it fresh.
struct NativeGrantSelection {
    account: String,
    credential_owner: String,
    connection: String,
}
struct ResolvedGrantBinding {
    fence: (String, String, i64),
    selected: Option<NativeGrantSelection>,
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
        Ok(self
            .resolve_grant_binding(id, credential, provider, request, now)
            .await?
            .fence)
    }
    async fn resolve_grant_binding(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: &ProviderGrantRequest,
        now: i64,
    ) -> Result<ResolvedGrantBinding, ApiError> {
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
            if generation != request.runtime_generation {
                tracing::warn!(workspace = %id, site = "provider_grant", reason = "request_generation_mismatch", generation, requested = request.runtime_generation, "Cloud runtime fenced");
            } else {
                super::log_fence("provider_grant", id, &row, credential, now);
            }
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
        // Preserve the old selected-config validation even for the initial agent,
        // whose normal binding path can return the launch connection immediately.
        let configured = agents::selected(config, provider.id(), account)?;
        let explicit = config["agentConnectionBindings"]
            .as_object()
            .is_some_and(|bindings| bindings.contains_key(provider.id()));
        if let Some((credential_owner, connection)) = self
            .bind_task_agent_tx(&mut tx, &row, provider.id())
            .await?
        {
            if explicit
                && configured.as_ref() != Some(&(credential_owner.clone(), connection.clone()))
            {
                return Err(ApiError::conflict("cloud_provider_grant_fence_mismatch"));
            }
            let selected = NativeGrantSelection {
                account: account.to_owned(),
                credential_owner,
                connection,
            };
            tx.commit().await?;
            let authority = self
                .authority
                .as_ref()
                .ok_or_else(|| ApiError::unavailable(provider.reconnecting()))?;
            let (incarnation, epoch) = authority
                .delivery_binding(
                    &selected.credential_owner,
                    &selected.connection,
                    provider.id(),
                )
                .await?;
            return Ok(ResolvedGrantBinding {
                fence: (thumb, incarnation, epoch),
                selected: Some(selected),
            });
        }
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
        Ok(ResolvedGrantBinding {
            fence: (thumb, incarnation, epoch),
            selected: None,
        })
    }
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="provider_grant"))]
    pub async fn provider_grant(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: ProviderGrantRequest,
        now: i64,
    ) -> Result<SealedProviderGrant, ApiError> {
        self.provider_grant_with_driver(id, credential, provider, request, now, None)
            .await
    }
    // The optional driver is private and permits deterministic authority concurrency
    // tests without replacing process environment or vendor responses in production.
    async fn provider_grant_with_driver(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: ProviderGrantRequest,
        now: i64,
        driver_override: Option<&dyn crate::connections::worker::AuthDriver>,
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
        // The binding and organization access reads are independent. Keep the
        // binding error first so an unauthenticated runtime sees the same result.
        let (resolved, scope) = tokio::join!(
            self.resolve_grant_binding(id, credential, provider, &request, now),
            self.scope_access(id, None, true),
        );
        let resolved = resolved?;
        scope?;
        let before = resolved.fence;
        let selected = resolved.selected;
        // Housekeeping only; it never changes this request's receipt outcome.
        let pool = self.pool.clone();
        tokio::spawn(async move {
            if let Err(error) = super::grant_receipts::cleanup_grant_receipts_on(&pool, now).await {
                tracing::warn!(code = %error.code, "provider grant receipt cleanup failed");
            }
        });
        let mut pending_receipt = None;
        let grant = if let Some(NativeGrantSelection {
            account,
            connection,
            credential_owner,
        }) = selected
        {
            let mut receipt = self
                .begin_grant_receipt(id, &account, &credential_owner, &request, &before, now)
                .await?;
            if let Some(cached) = receipt.cached.take() {
                return self
                    .checked_provider_grant(id, credential, provider, &request, before, cached)
                    .await;
            }
            let account = account.as_str();
            let connection = connection.as_str();
            let authority = self
                .authority
                .as_ref()
                .ok_or_else(|| ApiError::unavailable(provider.reconnecting()))?;
            let script = std::env::var_os("ZUSE_CLOUD_AUTH_DRIVER")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("/usr/local/lib/zuse/auth-driver.py"));
            let command_driver;
            let driver = if let Some(driver) = driver_override {
                driver
            } else {
                command_driver = crate::connections::worker::CommandDriver::new(&script)
                    .map_err(|_| ApiError::unavailable(provider.reconnecting()))?;
                &command_driver
            };
            let access = authority
                .credential(
                    &credential_owner,
                    connection,
                    provider.id(),
                    matches!(
                        request.reason,
                        zuse_protocol::cloud_auth::GrantReason::Unauthorized
                    ),
                    driver,
                )
                .await
                .map_err(|error| {
                    ApiError::unavailable(if error.code == "agent_connection_refresh_pending" {
                        provider.reconnecting()
                    } else {
                        provider.reconnect_required()
                    })
                })?;
            let now = chrono::Utc::now().timestamp_millis();
            let expiry = access
                .expires_at
                .unwrap_or(now + 15 * 60_000)
                .min(now + 15 * 60_000);
            if expiry <= now + 60_000 {
                return Err(ApiError::unavailable(provider.reconnect_required()));
            }
            if request
                .previous_provider_account_id
                .as_deref()
                .or(request.previous_chatgpt_account_id.as_deref())
                .is_some_and(|previous| Some(previous) != access.account_id.as_deref())
            {
                return Err(ApiError::conflict("provider_grant_account_changed"));
            }
            receipt.expires_at = expiry;
            pending_receipt = Some(receipt);
            let mut grant = SealedProviderGrant {
                protocol_version: 1,
                provider_id: Some(provider),
                request_id: request.request_id.clone(),
                key_thumbprint: before.0.clone(),
                authority_incarnation_id: connection.into(),
                authority_epoch: access.authority_epoch,
                wrapped_key: String::new(),
                iv: String::new(),
                ciphertext: String::new(),
                tag: String::new(),
            };
            let payload=zeroize::Zeroizing::new(json!({"zuseAccountId":account,"workspaceId":id,"runtimeGeneration":request.runtime_generation,"keyThumbprint":grant.key_thumbprint,"authorityIncarnationId":connection,"authorityEpoch":access.authority_epoch,"providerId":provider,"method":access.method,"credentialKind":access.kind,"secret":access.secret.as_str(),"providerAccountId":access.account_id,"chatgptAccountId":if provider == CloudAuthProvider::Codex {access.account_id.as_deref()} else {None},"issuedAt":now,"expiresAt":expiry}).to_string());
            crate::crypto::seal_provider_grant(
                &request.credential_public_jwk,
                &mut grant,
                &payload,
            )
            .map_err(|_| ApiError::unavailable("provider_grant_encryption_failed"))?;
            grant
        } else {
            let issuer = self
                .grant_issuer
                .as_ref()
                .ok_or_else(|| ApiError::unavailable(provider.reconnecting()))?;
            issuer.issue(id, provider, credential, &request).await?
        };
        let grant = self
            .checked_provider_grant(id, credential, provider, &request, before, grant)
            .await?;
        if let Some(receipt) = pending_receipt {
            self.finish_grant_receipt(&receipt, &grant, chrono::Utc::now().timestamp_millis())
                .await?;
        }
        Ok(grant)
    }
    async fn checked_provider_grant(
        &self,
        id: &str,
        credential: &str,
        provider: CloudAuthProvider,
        request: &ProviderGrantRequest,
        before: (String, String, i64),
        grant: SealedProviderGrant,
    ) -> Result<SealedProviderGrant, ApiError> {
        // Renewal may outlive organization membership or sharing-policy changes.
        // Revalidate after authority work without holding any workspace SQL lock.
        // Never return a grant completed after runtime revocation, authority rotation or writer handoff.
        // Both reads start after the authority work finished, so run them together.
        let (scope, after) = tokio::join!(
            self.scope_access(id, None, true),
            self.grant_binding(
                id,
                credential,
                provider,
                &request,
                chrono::Utc::now().timestamp_millis(),
            ),
        );
        scope?;
        let after = after?;
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

#[cfg(test)]
mod organization_binding_tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rsa::traits::PublicKeyParts;
    #[tokio::test]
    #[ignore = "requires explicit isolated localhost PostgreSQL"]
    async fn organization_grants_use_pinned_personal_connection_without_borrowing_admin() {
        let url =
            std::env::var("ZUSE_CLOUD_TEST_DATABASE_URL").expect("isolated PostgreSQL required");
        assert!(matches!(
            reqwest::Url::parse(&url).unwrap().host_str(),
            Some("localhost" | "127.0.0.1" | "::1")
        ));
        let admin = PgPool::connect(&url).await.unwrap();
        let schema = format!("binding_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |connection, _| {
                let search = search.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={search}"))
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0001_control_plane.sql"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE api_cloud_workspaces(workspace_id text PRIMARY KEY,account_id text,provider_sandbox_id text,runtime_credential_hash text,state text,desired_state text,request_config jsonb);CREATE TABLE cloud_agent_connections(connection_id text PRIMARY KEY,owner_id text,agent text,method text,state text,credential_ciphertext text,version bigint,authority_epoch bigint,expires_at bigint);").execute(&pool).await.unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[93; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([93;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
        let master = URL_SAFE_NO_PAD.encode([94; 32]);
        let authority = Arc::new(crate::connections::ConnectionControl::from_pool(
            pool.clone(),
            MasterKey::parse(&master).unwrap(),
            vec!["user-member".into()],
        ));
        let control = RuntimeControl::from_pool(
            pool.clone(),
            MasterKey::parse(&master).unwrap(),
            TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap(),
        )
        .with_credential_authority(authority);
        let rsa = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        let public=json!({"kty":"RSA","n":URL_SAFE_NO_PAD.encode(rsa.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(rsa.e().to_bytes_be())}).to_string();
        let thumb = credential_thumbprint(&public).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let mut config = json!({"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"runtimeGeneration":1,"gatewayEpoch":1,"runtimeBootstrapReceipt":{"workspaceId":"task","generation":1,"gatewayEpoch":1},"runtimeCredentialExpiresAtMs":now+600000,"runtimeCredentialKeyThumbprint":thumb,"runtimeSigningKeyThumbprint":"signing-thumb","providerAuthMode":"broker-v1","rustActorId":"user-member","agentConnectionId":"personal-connection","agentConnectionOwnerId":"user-member","rustRuntimeStorage":{"storageIncarnationId":"disk"},"rustStorageEnrollment":{"phase":"committed","generation":1,"sourceSandboxId":"sandbox","storageIncarnationId":"disk","credentialKeyThumbprint":thumb,"signingKeyThumbprint":"signing-thumb"}});
        sqlx::query("INSERT INTO zuse_rust_control_owners VALUES('workspace','task','rust',1,$1)")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO api_cloud_workspaces VALUES('task','organization:team','sandbox',$1,'ready','ready',$2)").bind(hash("runtime")).bind(&config).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO cloud_agent_connections VALUES('personal-connection','user-member','codex','api-key','connected','private-envelope-not-opened-for-binding',1,3,NULL)").execute(&pool).await.unwrap();
        let request = ProviderGrantRequest {
            protocol_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            runtime_generation: 1,
            credential_public_jwk: public,
            reason: zuse_protocol::cloud_auth::GrantReason::Initial,
            previous_provider_account_id: None,
            previous_chatgpt_account_id: None,
        };
        let binding = control
            .grant_binding("task", "runtime", CloudAuthProvider::Codex, &request, now)
            .await
            .unwrap();
        assert_eq!(binding.1, "personal-connection");
        assert_eq!(binding.2, 3);
        config["agentConnectionOwnerId"] = json!("installing-admin");
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1")
            .bind(&config)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            control
                .grant_binding("task", "runtime", CloudAuthProvider::Codex, &request, now)
                .await
                .unwrap_err()
                .code,
            "agent_connection_owner_mismatch"
        );
        config["agentConnectionOwnerId"] = json!("user-member");
        config["rustActorId"] = json!("different-actor");
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1")
            .bind(&config)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            control
                .grant_binding("task", "runtime", CloudAuthProvider::Codex, &request, now)
                .await
                .unwrap_err()
                .code,
            "agent_connection_owner_mismatch"
        );
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
}

#[cfg(test)]
#[path = "grant_scope_tests.rs"]
mod scope_refresh_tests;

#[cfg(test)]
#[path = "grant_receipt_tests.rs"]
mod receipt_tests;

#[cfg(test)]
#[path = "grant_latency_tests.rs"]
mod latency_tests;
