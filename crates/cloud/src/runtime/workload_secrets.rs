//! Project-scoped workload secret authority. Values never leave this module
//! except inside an enrolled runtime's recipient-key sealed grant.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroize;
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveWorkloadSecret {
    pub name: String,
    pub value: String,
    pub expected_revision: i64,
}
impl Drop for SaveWorkloadSecret {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadGrantRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub runtime_generation: i64,
    pub credential_public_jwk: String,
    pub bindings: Vec<String>,
}
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_uppercase() || (i > 0 && b.is_ascii_digit()))
        && !name.starts_with("ZUSE_")
        && !name.starts_with("DYLD_")
        && !name.starts_with("LD_")
        && ![
            "PATH",
            "HOME",
            "SHELL",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "BASH_ENV",
            "ENV",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "XAI_API_KEY",
            "GROK_API_KEY",
            "GROK_AUTH_PROVIDER_COMMAND",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_HOST",
            "GH_CONFIG_DIR",
            "GH_DEBUG",
            "CLOUD_DATA_ENCRYPTION_KEY",
            "CLOUD_CREDENTIAL_VAULT_KEY",
            "WORKOS_API_KEY",
        ]
        .contains(&name)
        // Every cloud-computer provider key, so a new provider is reserved by default.
        && !zuse_protocol::cloud_computers::ProviderId::is_api_key_env(name)
}
fn context(owner: &str, project: &str, id: &str, revision: i64) -> String {
    format!("workload-secret:{owner}:{project}:{id}:{revision}")
}
impl RuntimeControl {
    pub(super) async fn validate_workload_bindings_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        owner: &str,
        project: &str,
        bindings: &[String],
    ) -> Result<(), ApiError> {
        if bindings.len() > 64 {
            return Err(ApiError::bad_request("invalid_workload_secret_binding"));
        }
        let enabled: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_workload_secrets') IS NOT NULL")
                .fetch_one(&mut **tx)
                .await?;
        if !enabled {
            return Err(ApiError::unavailable(
                "cloud_workload_secret_vault_unavailable",
            ));
        }
        let rows:Vec<(String,i64,String)>=sqlx::query_as("SELECT secret_id,revision,ciphertext FROM zuse_rust_workload_secrets WHERE owner_id=$1 AND project_id=$2 AND secret_id=ANY($3) AND revoked_at IS NULL FOR SHARE").bind(owner).bind(project).bind(bindings).fetch_all(&mut **tx).await?;
        if rows.len() != bindings.len() {
            return Err(ApiError::conflict("workload_secret_missing_or_revoked"));
        }
        self.validate_workload_binding_rows(owner,project,bindings,&rows.into_iter().map(|(id,revision,ciphertext)|json!({"secret_id":id,"revision":revision,"ciphertext":ciphertext})).collect::<Vec<_>>())
    }
    pub(super) fn validate_workload_binding_rows(
        &self,
        owner: &str,
        project: &str,
        bindings: &[String],
        rows: &[Value],
    ) -> Result<(), ApiError> {
        if bindings.len() > 64 {
            return Err(ApiError::bad_request("invalid_workload_secret_binding"));
        }
        if rows.len() != bindings.len() {
            return Err(ApiError::conflict("workload_secret_missing_or_revoked"));
        }
        let mut bytes = 0usize;
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let id = row["secret_id"]
                .as_str()
                .ok_or_else(|| ApiError::conflict("workload_secret_missing_or_revoked"))?;
            if !seen.insert(id) || !bindings.iter().any(|binding| binding == id) {
                return Err(ApiError::conflict("workload_secret_missing_or_revoked"));
            }
            let revision = row["revision"]
                .as_i64()
                .ok_or_else(|| ApiError::conflict("workload_secret_missing_or_revoked"))?;
            let ciphertext = row["ciphertext"]
                .as_str()
                .ok_or_else(|| ApiError::conflict("workload_secret_missing_or_revoked"))?;
            let value = self
                .master
                .open_api(&context(owner, project, id, revision), ciphertext)
                .map_err(|_| ApiError::unavailable("workload_secret_decryption_unavailable"))?;
            bytes += value.len();
        }
        if bytes > 24 * 1024 {
            return Err(ApiError::conflict("workload_secret_total_too_large"));
        }
        Ok(())
    }
    async fn workload_vault_ready(&self) -> Result<(), ApiError> {
        let enabled: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_workload_secrets') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !enabled {
            return Err(ApiError::unavailable(
                "cloud_workload_secret_vault_unavailable",
            ));
        }
        Ok(())
    }
    pub async fn workload_secrets(
        &self,
        access: &crate::access::Access,
        project: &str,
    ) -> Result<Value, ApiError> {
        self.workload_vault_ready().await?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true)").bind(&access.owner_id).bind(project).fetch_one(&self.pool).await?;
        if !exists {
            return Err(ApiError::not_found("cloud_project_not_found"));
        }
        let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('secretId',secret_id,'name',environment_name,'revision',revision,'revoked',revoked_at IS NOT NULL,'updatedAt',updated_at) FROM zuse_rust_workload_secrets WHERE owner_id=$1 AND project_id=$2 ORDER BY environment_name,secret_id").bind(&access.owner_id).bind(project).fetch_all(&self.pool).await?;
        Ok(json!({"secrets":rows}))
    }
    pub async fn save_workload_secret(
        &self,
        access: &crate::access::Access,
        project: &str,
        request: SaveWorkloadSecret,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.workload_vault_ready().await?;
        if !valid_name(&request.name)
            || request.value.is_empty()
            || request.value.len() > 8192
            || request.value.contains('\0')
            || request.expected_revision < 0
            || request.expected_revision == i64::MAX
        {
            return Err(ApiError::bad_request("invalid_workload_secret"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!(
                "workload-secret:{}:{project}:{}",
                access.owner_id, request.name
            ))
            .execute(&mut *tx)
            .await?;
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true)").bind(&access.owner_id).bind(project).fetch_one(&mut *tx).await?;
        if !exists {
            return Err(ApiError::not_found("cloud_project_not_found"));
        }
        let current:Option<(String,i64)>=sqlx::query_as("SELECT secret_id,revision FROM zuse_rust_workload_secrets WHERE owner_id=$1 AND project_id=$2 AND environment_name=$3 FOR UPDATE").bind(&access.owner_id).bind(project).bind(&request.name).fetch_optional(&mut *tx).await?;
        if current.as_ref().map(|(_, version)| *version).unwrap_or(0) != request.expected_revision {
            return Err(ApiError::conflict("workload_secret_revision_changed"));
        }
        let id = current
            .map(|(id, _)| id)
            .unwrap_or_else(|| format!("secret_{}", uuid::Uuid::new_v4().simple()));
        let revision = request.expected_revision + 1;
        let ciphertext = self
            .master
            .seal_api(
                &context(&access.owner_id, project, &id, revision),
                &request.value,
            )
            .map_err(|_| ApiError::unavailable("workload_secret_encryption_unavailable"))?;
        sqlx::query("INSERT INTO zuse_rust_workload_secrets(secret_id,owner_id,project_id,environment_name,revision,ciphertext,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$7) ON CONFLICT(secret_id) DO UPDATE SET revision=EXCLUDED.revision,ciphertext=EXCLUDED.ciphertext,revoked_at=NULL,updated_at=EXCLUDED.updated_at").bind(&id).bind(&access.owner_id).bind(project).bind(&request.name).bind(revision).bind(ciphertext).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(
            json!({"secretId":id,"name":request.name,"revision":revision,"revoked":false,"updatedAt":now}),
        )
    }
    pub async fn revoke_workload_secret(
        &self,
        access: &crate::access::Access,
        project: &str,
        id: &str,
        revision: i64,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.workload_vault_ready().await?;
        if revision < 1 || revision == i64::MAX {
            return Err(ApiError::bad_request("invalid_workload_secret_revision"));
        }
        let result=sqlx::query("UPDATE zuse_rust_workload_secrets SET ciphertext=NULL,revoked_at=$5,updated_at=$5,revision=revision+1 WHERE owner_id=$1 AND project_id=$2 AND secret_id=$3 AND revision=$4 AND revoked_at IS NULL").bind(&access.owner_id).bind(project).bind(id).bind(revision).bind(now).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::conflict("workload_secret_revision_changed"));
        }
        Ok(json!({"secretId":id,"revision":revision+1,"revoked":true}))
    }
    pub async fn workload_grant(
        &self,
        id: &str,
        credential: &str,
        request: WorkloadGrantRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.workload_vault_ready().await?;
        if request.protocol_version != 1
            || uuid::Uuid::parse_str(&request.request_id).is_err()
            || request.bindings.len() > 64
            || request.credential_public_jwk.len() > 16384
            || request.bindings.iter().collect::<BTreeSet<_>>().len() != request.bindings.len()
        {
            return Err(ApiError::bad_request("invalid_workload_grant_request"));
        }
        self.scope_access(id, None, true).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        let thumb = credential_thumbprint(&request.credential_public_jwk)
            .map_err(|_| ApiError::bad_request("invalid_workspace_key"))?;
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
            || row["request_config"]["runtimeCredentialKeyThumbprint"] != thumb
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if row["request_config"]["rustWorkloadSecretsVersion"] != 1 {
            return Err(ApiError::conflict(
                "workload_secret_runtime_update_required",
            ));
        }
        let allowed = row["request_config"]["workloadSecretBindings"].as_array();
        if request.bindings.iter().any(|id| {
            !allowed.is_some_and(|allowed| allowed.iter().any(|value| value.as_str() == Some(id)))
        }) {
            return Err(ApiError::forbidden("workload_secret_not_selected"));
        }
        let owner = required(&row, "account_id")?;
        let project = required(&row, "project_id")?;
        let mut values = BTreeMap::new();
        let mut versions = BTreeMap::new();
        for binding in &request.bindings {
            let secret:Option<(String,i64,String)>=sqlx::query_as("SELECT environment_name,revision,ciphertext FROM zuse_rust_workload_secrets WHERE owner_id=$1 AND project_id=$2 AND secret_id=$3 AND revoked_at IS NULL FOR SHARE").bind(owner).bind(project).bind(binding).fetch_optional(&mut *tx).await?;
            let (name, revision, ciphertext) =
                secret.ok_or_else(|| ApiError::conflict("workload_secret_revoked"))?;
            if !valid_name(&name) {
                return Err(ApiError::conflict("workload_secret_name_invalid"));
            }
            let bytes = self
                .master
                .open_api(&context(owner, project, binding, revision), &ciphertext)
                .map_err(|_| ApiError::unavailable("workload_secret_decryption_unavailable"))?;
            let value = std::str::from_utf8(&bytes)
                .map_err(|_| ApiError::unavailable("workload_secret_decryption_unavailable"))?;
            if values.values().map(String::len).sum::<usize>() + value.len() > 24 * 1024 {
                return Err(ApiError::conflict("workload_secret_total_too_large"));
            }
            values.insert(name, value.to_owned());
            versions.insert(binding.clone(), revision);
        }
        let expires = now + 5 * 60_000;
        let encoded=zeroize::Zeroizing::new(serde_json::to_string(&json!({"protocolVersion":1,"requestId":request.request_id,"workspaceId":id,"ownerId":owner,"runtimeGeneration":generation,"credentialKeyThumbprint":thumb,"expiresAtMs":expires,"bindings":versions,"environment":values})).map_err(|_|ApiError::unavailable("workload_grant_unavailable"))?);
        for value in values.values_mut() {
            value.zeroize();
        }
        let sealed = crate::crypto::seal_runtime_secret(&request.credential_public_jwk, &encoded)
            .map_err(|_| ApiError::unavailable("workload_grant_unavailable"))?;
        tx.commit().await?;
        Ok(
            json!({"protocolVersion":1,"requestId":request.request_id,"runtimeGeneration":generation,"expiresAtMs":expires,"sealedGrant":sealed}),
        )
    }
}

#[cfg(test)]
mod tests;
