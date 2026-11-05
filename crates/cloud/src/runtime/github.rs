//! Git credentials are scoped by the immutable task actor and repository, then
//! sealed to the enrolled runtime key. Never serialize the parent authorization.
use super::*;
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitGrantRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub runtime_generation: i64,
    pub credential_public_jwk: String,
}
impl RuntimeControl {
    async fn github_binding(
        &self,
        id: &str,
        credential: &str,
        request: &GitGrantRequest,
        now: i64,
    ) -> Result<(String, String, String, String, i64), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if blocked(&row)
            || row["desired_state"] != "ready"
            || generation != request.runtime_generation
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|value| !equal(value, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
            || !enrollment_matches(&row)
            || !git_storage_prepared(&row)
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let thumb = credential_thumbprint(&request.credential_public_jwk)
            .map_err(|_| ApiError::bad_request("invalid_workspace_key"))?;
        if row["request_config"]["runtimeCredentialKeyThumbprint"] != thumb {
            return Err(ApiError::unauthorized(
                "runtime_credential_key_binding_mismatch",
            ));
        }
        let owner = required(&row, "account_id")?.to_owned();
        let actor = row["request_config"]["rustActorId"]
            .as_str()
            .or_else(|| (!owner.starts_with("organization:")).then_some(owner.as_str()))
            .ok_or_else(|| ApiError::conflict("github_task_actor_migration_required"))?
            .to_owned();
        if row["request_config"]["gitAuthMode"] != "broker-v1" {
            return Err(ApiError::conflict(
                "github_task_credential_binding_required",
            ));
        }
        let github_user_id = row["request_config"]["rustGithubUserId"]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or_else(|| ApiError::conflict("github_task_credential_binding_required"))?;
        let url = if let Some(url) =
            row["request_config"]["rustRepositoryCheckout"]["repositoryUrl"].as_str()
        {
            url.to_owned()
        } else {
            let project = required(&row, "project_id")?;
            sqlx::query_scalar::<_,String>("SELECT repository_url FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2").bind(&owner).bind(project).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("github_task_repository_missing"))?
        };
        let url = reqwest::Url::parse(&url)
            .map_err(|_| ApiError::conflict("github_task_repository_invalid"))?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiError::conflict("github_task_repository_invalid"));
        }
        let path = url.path().trim_start_matches('/');
        let repository = path.strip_suffix(".git").unwrap_or(path).to_owned();
        tx.commit().await?;
        Ok((owner, actor, repository, thumb, github_user_id))
    }
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="github_grant"))]
    pub async fn github_grant(
        &self,
        authority: &crate::github::native::NativeGithub,
        id: &str,
        credential: &str,
        request: GitGrantRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        if request.protocol_version != 1
            || uuid::Uuid::parse_str(&request.request_id).is_err()
            || request.credential_public_jwk.len() > 16 * 1024
        {
            return Err(ApiError::bad_request("invalid_github_grant_request"));
        }
        let before = self.github_binding(id, credential, &request, now).await?;
        // The runtime is authenticated by `before`. Access, identity and the
        // narrowed GitHub token are independent; a failed check discards the
        // token before it is sealed, and the `after` fence still runs last.
        let (scope, connected, grant) = tokio::join!(
            self.scope_access(id, None, true),
            authority.connected_user_id(&before.1),
            authority.scoped_git(&before.0, &before.1, &before.2),
        );
        scope?;
        if connected? != before.4 {
            return Err(ApiError::conflict("github_account_changed"));
        }
        let grant = grant?;
        let after = self
            .github_binding(
                id,
                credential,
                &request,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        if grant.github_user_id != before.4 {
            return Err(ApiError::conflict("github_account_changed"));
        }
        if before != after {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let plaintext = zeroize::Zeroizing::new(
            serde_json::to_string(&git_grant_payload(id, &request, &before, &grant))
                .map_err(|_| ApiError::unavailable("github_grant_unavailable"))?,
        );
        let sealed = crate::crypto::seal_runtime_secret(&request.credential_public_jwk, &plaintext)
            .map_err(|_| ApiError::unavailable("github_grant_unavailable"))?;
        Ok(
            json!({"protocolVersion":1,"requestId":request.request_id,"runtimeGeneration":request.runtime_generation,"expiresAtMs":grant.authority_expires_at,"authorityExpiresAtMs":grant.authority_expires_at,"providerExpiresAtMs":grant.provider_expires_at,"sealedGrant":sealed}),
        )
    }
}

fn git_storage_prepared(row: &Value) -> bool {
    let config = &row["request_config"];
    let storage = &config["rustStorageEnrollment"];
    matches!(storage["phase"].as_str(), Some("prepared" | "committed"))
        && storage["generation"].as_i64() == Some(config["runtimeGeneration"].as_i64().unwrap_or(1))
        && storage["sourceSandboxId"].as_str().is_some()
        && storage["sourceSandboxId"] == row["provider_sandbox_id"]
        && storage["storageIncarnationId"].as_str().is_some()
        && storage["storageIncarnationId"] == config["rustRuntimeStorage"]["storageIncarnationId"]
        && storage["credentialKeyThumbprint"].as_str().is_some()
        && storage["credentialKeyThumbprint"] == config["runtimeCredentialKeyThumbprint"]
        && storage["signingKeyThumbprint"].as_str().is_some()
        && storage["signingKeyThumbprint"] == config["runtimeSigningKeyThumbprint"]
}

// expiresAtMs is the v1 guest-compatible local authorization deadline, not the
// upstream bearer expiry. Both values are authenticated inside the sealed JWE.
fn git_grant_payload(
    id: &str,
    request: &GitGrantRequest,
    binding: &(String, String, String, String, i64),
    grant: &crate::github::native::ScopedGitCredential,
) -> Value {
    json!({"protocolVersion":1,"requestId":request.request_id,"workspaceId":id,"ownerId":binding.0,"actorId":binding.1,"githubUserId":binding.4,"repository":binding.2,"runtimeGeneration":request.runtime_generation,"credentialKeyThumbprint":binding.3,"expiresAtMs":grant.authority_expires_at,"authorityExpiresAtMs":grant.authority_expires_at,"providerExpiresAtMs":grant.provider_expires_at,"token":grant.token.as_str(),"login":grant.login,"email":grant.email})
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use aes_gcm::{
        Aes256Gcm, KeyInit, Nonce,
        aead::{Aead, Payload},
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rsa::{Oaep, RsaPrivateKey, pkcs8::DecodePrivateKey};
    #[test]
    fn scoped_git_grant_seals_provider_and_authority_deadlines_with_identity() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/cloud-crypto.json")).unwrap();
        let request = GitGrantRequest {
            protocol_version: 1,
            request_id: "req".into(),
            runtime_generation: 7,
            credential_public_jwk: fixture["runtimePublicJwk"].to_string(),
        };
        let binding = (
            "organization:team".into(),
            "member".into(),
            "team/private".into(),
            "thumb".into(),
            42,
        );
        let grant = crate::github::native::ScopedGitCredential {
            github_user_id: 42,
            token: zeroize::Zeroizing::new("test-scoped-token".into()),
            authority_expires_at: 3_600_000,
            provider_expires_at: 28_800_000,
            login: "member".into(),
            email: "42+member@users.noreply.github.com".into(),
        };
        let payload = git_grant_payload("task", &request, &binding, &grant);
        let sealed = crate::crypto::seal_runtime_secret(
            &request.credential_public_jwk,
            &payload.to_string(),
        )
        .unwrap();
        let parts: Vec<_> = sealed.split('.').collect();
        let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).unwrap();
        let key = RsaPrivateKey::from_pkcs8_der(&decode(
            fixture["runtimePrivatePkcs8"].as_str().unwrap(),
        ))
        .unwrap();
        let cek = zeroize::Zeroizing::new(
            key.decrypt(Oaep::new::<sha2::Sha256>(), &decode(parts[1]))
                .unwrap(),
        );
        let mut ciphertext = decode(parts[3]);
        ciphertext.extend(decode(parts[4]));
        let cipher = Aes256Gcm::new_from_slice(&cek).unwrap();
        let opened = cipher
            .decrypt(
                Nonce::from_slice(&decode(parts[2])),
                Payload {
                    msg: &ciphertext,
                    aad: parts[0].as_bytes(),
                },
            )
            .unwrap();
        let decoded: Value = serde_json::from_slice(&opened).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded["expiresAtMs"], decoded["authorityExpiresAtMs"]);
        assert_eq!(decoded["providerExpiresAtMs"], 28_800_000);
        assert_eq!(decoded["actorId"], "member");
        assert_eq!(decoded["repository"], "team/private");
        assert_eq!(decoded["runtimeGeneration"], 7);
        let mut wrong_aad = parts[0].as_bytes().to_vec();
        wrong_aad.push(b'x');
        assert!(
            cipher
                .decrypt(
                    Nonce::from_slice(&decode(parts[2])),
                    Payload {
                        msg: &ciphertext,
                        aad: &wrong_aad
                    }
                )
                .is_err()
        );
    }
}
