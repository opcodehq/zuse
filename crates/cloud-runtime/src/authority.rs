//! Runtime-only HTTP grants. Redirects and error bodies never expose credentials.
use crate::{Identity, IdentityBinding, now_ms};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::time::Duration;
use zuse_protocol::{
    cloud::{CloudRuntimeBootstrap, CloudRuntimeBootstrapAck},
    cloud_gateway::{RUST_PROTOCOL, RUST_RPC},
};
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub retryable: bool,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code)
    }
}
impl std::error::Error for Failure {}
#[derive(Clone)]
pub struct Authority {
    client: reqwest::Client,
    origin: reqwest::Url,
}
impl Authority {
    pub fn new(origin: &str) -> Result<Self> {
        let parsed = reqwest::Url::parse(origin)?;
        let loopback = parsed.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            (parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback))
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.fragment().is_none(),
            "runtime_api_origin_invalid"
        );
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            origin: parsed,
        })
    }
    pub async fn plugin_tools(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        request: &zuse_protocol::plugins::PluginToolRequest,
    ) -> Result<Value> {
        ensure!(
            bootstrap.runtime_credential_expires_at > now_ms(),
            "runtime_plugin_credential_expired"
        );
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?
            .extend(["v1", "plugins", "runtime", &bootstrap.workspace_id, "tools"]);
        let mut response = self
            .client
            .post(url)
            .bearer_auth(&bootstrap.runtime_credential)
            .timeout(Duration::from_secs(60))
            .json(request)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("runtime_plugins_unavailable"))?;
        ensure!(response.status().is_success(), "runtime_plugins_rejected");
        let mut body = Vec::new();
        while let Some(part) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("runtime_plugins_unavailable"))?
        {
            ensure!(
                body.len() + part.len() <= 8 * 1024 * 1024,
                "runtime_plugins_response_too_large"
            );
            body.extend_from_slice(&part);
        }
        serde_json::from_slice(&body)
            .map_err(|_| anyhow::anyhow!("runtime_plugins_invalid_response"))
    }
    pub async fn asset(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        expected: &zuse_protocol::cloud_assets::Asset,
    ) -> Result<(zuse_protocol::cloud_assets::Asset, Vec<u8>)> {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?
            .extend([
                "v1",
                "cloud",
                "workspaces",
                &bootstrap.workspace_id,
                "runtime",
                "attachments",
                &expected.asset_id,
            ]);
        let mut response = self
            .client
            .get(url)
            .bearer_auth(&bootstrap.runtime_credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("runtime_asset_unavailable"))?;
        ensure!(response.status().is_success(), "runtime_asset_unavailable");
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("runtime_asset_unavailable"))?
        {
            ensure!(
                body.len() + chunk.len() <= 32 * 1024 * 1024,
                "runtime_asset_response_too_large"
            );
            body.extend_from_slice(&chunk);
        }
        let download: zuse_protocol::cloud_assets::Download =
            serde_json::from_slice(&body).map_err(|_| anyhow::anyhow!("runtime_asset_invalid"))?;
        let mut identity = expected.clone();
        // Original launch envelopes predate a public digest field. The authority
        // authenticates that immutable object's digest before returning its bytes.
        if identity.sha256.is_empty() {
            identity.sha256 = download.asset.sha256.clone();
        }
        ensure!(
            download.asset == identity
                && expected.size_bytes > 0
                && expected.size_bytes <= zuse_protocol::cloud_assets::MAX_BYTES,
            "runtime_asset_identity_mismatch"
        );
        let bytes = URL_SAFE_NO_PAD
            .decode(&download.bytes)
            .map_err(|_| anyhow::anyhow!("runtime_asset_invalid"))?;
        ensure!(
            bytes.len() == expected.size_bytes
                && URL_SAFE_NO_PAD.encode(Sha256::digest(&bytes)) == download.asset.sha256,
            "runtime_asset_integrity_failed"
        );
        Ok((download.asset, bytes))
    }
    pub async fn lifecycle(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        request: zuse_protocol::cloud_lifecycle::RuntimeRequest,
    ) -> Result<Option<zuse_protocol::cloud_lifecycle::Directive>> {
        let response = self
            .post(
                &bootstrap.workspace_id,
                &["runtime", "lifecycle"],
                &bootstrap.runtime_credential,
                &serde_json::to_value(request)?,
            )
            .await?;
        let directive: Option<zuse_protocol::cloud_lifecycle::Directive> =
            serde_json::from_value(response["directive"].clone())?;
        if let Some(d) = &directive {
            ensure!(
                d.generation == bootstrap.runtime_generation
                    && Some(d.storage_incarnation_id.as_str())
                        == bootstrap
                            .runtime_storage
                            .as_ref()
                            .map(|v| v.storage_incarnation_id.as_str()),
                "runtime_lifecycle_identity_mismatch"
            );
        }
        Ok(directive)
    }
    pub fn origin(&self) -> String {
        self.origin.origin().ascii_serialization()
    }
    pub(crate) async fn post(
        &self,
        workspace: &str,
        suffix: &[&str],
        credential: &str,
        body: &Value,
    ) -> Result<Value> {
        self.send(
            reqwest::Method::POST,
            workspace,
            suffix,
            credential,
            Some(body),
        )
        .await
    }
    /// Same bearer, redirect and error handling as [`Self::post`], without a body.
    pub(crate) async fn get(
        &self,
        workspace: &str,
        suffix: &[&str],
        credential: &str,
    ) -> Result<Value> {
        self.send(reqwest::Method::GET, workspace, suffix, credential, None)
            .await
    }
    async fn send(
        &self,
        method: reqwest::Method,
        workspace: &str,
        suffix: &[&str],
        credential: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let name = match suffix {
            ["runtime", "bootstrap"] => Some("bootstrap-http"),
            ["runtime", "bootstrap", "ack"] => Some("bootstrap-ack-http"),
            ["runtime", "storage"] => Some("storage-http"),
            ["runtime", "preparation", "ingress"] => Some("preparation-ingress-http"),
            ["runtime", "providers", _, "grant"] => Some("agent-credential-http"),
            ["ready"] => Some("readiness-http"),
            _ => None,
        };
        let mut phase = name.map(|name| crate::startup::Phase::new(workspace, name));
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?
            .clear()
            .extend(["v1", "cloud", "workspaces", workspace])
            .extend(suffix.iter().copied());
        let request = self
            .client
            .request(method, url)
            // Native OAuth refresh may take fifteen seconds plus checkpoint writes.
            // Keep unrelated control calls at the original ten-second budget.
            .timeout(Duration::from_secs(
                if suffix.len() == 4
                    && suffix[0] == "runtime"
                    && suffix[1] == "providers"
                    && suffix[3] == "grant"
                {
                    35
                } else {
                    10
                },
            ))
            .bearer_auth(credential);
        let request = match body {
            Some(body) => request.json(body),
            None => request,
        };
        let mut response = request.send().await.map_err(|_| Failure {
            code: "runtime_authority_unreachable",
            retryable: true,
        })?;
        let status = response.status();
        ensure!(
            response
                .content_length()
                .is_none_or(|l| l <= 8 * 1024 * 1024),
            "runtime_authority_response_invalid"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure {
            code: "runtime_authority_unreachable",
            retryable: true,
        })? {
            ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "runtime_authority_response_invalid"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            const CODES: &[&str] = &[
                "cloud_command_mailbox_disabled",
                "cloud_workspace_runtime_not_ready",
                "stale-runtime-lease",
                "runtime-storage-replaced",
                "workspace_runtime_fenced",
                "workspace_runtime_revoked",
                "workspace_bootstrap_rejected",
                "workspace_bootstrap_key_mismatch",
                "workspace_rpc_protocol_mismatch",
                "runtime_storage_enrollment_required",
                "runtime_storage_recovery_required",
                "runtime_storage_enrollment_rejected",
                "cloud_workspace_launch_failed",
                "cloud_workspace_launch_intent_unavailable",
                "launch_intent_receipt_rejected",
                "github_runtime_update_required",
                "project_cache_runtime_update_required",
            ];
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let code = CODES
                .iter()
                .copied()
                .find(|code| body["error"].as_str() == Some(*code))
                .unwrap_or(if [401, 403, 409].contains(&status.as_u16()) {
                    "runtime_authority_fenced"
                } else {
                    "runtime_authority_unavailable"
                });
            return Err(Failure {
                code,
                retryable: status.is_server_error() || status.as_u16() == 429,
            }
            .into());
        }
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("runtime_authority_response_invalid"))?;
        if let Some(phase) = &mut phase {
            phase.complete();
        }
        Ok(value)
    }
    pub async fn enroll(
        &self,
        identity: &mut Identity,
        binding: &IdentityBinding,
        boot: Option<&str>,
    ) -> Result<CloudRuntimeBootstrap> {
        ensure!(
            binding.api_url == self.origin(),
            "runtime_api_origin_mismatch"
        );
        if identity.bootstrap().is_none() {
            let response = self
                .post(
                    &binding.workspace,
                    &["runtime", "bootstrap"],
                    boot.ok_or_else(|| anyhow::anyhow!("runtime_boot_token_required"))?,
                    &serde_json::to_value(identity.request())?,
                )
                .await?;
            let response: CloudRuntimeBootstrap = serde_json::from_value(response)
                .map_err(|_| anyhow::anyhow!("runtime_authority_response_invalid"))?;
            self.validate(&response, binding)?;
            identity.commit_bootstrap(response, binding.generation)?;
        }
        let current = identity
            .bootstrap()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        self.validate(current, binding)?;
        let gateway_ack = current.extra.get("bootstrapAckMode") == Some(&json!("gateway-v1"));
        if identity.has_pending_renewal()
            || current.runtime_credential_expires_at <= now_ms() + 60000
        {
            self.renew(identity).await?;
        }
        if !identity.acknowledged() && !gateway_ack {
            let current = identity
                .bootstrap()
                .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
            let ack = self
                .post(
                    &binding.workspace,
                    &["runtime", "bootstrap", "ack"],
                    &current.runtime_credential,
                    &serde_json::to_value(CloudRuntimeBootstrapAck {
                        runtime_generation: current.runtime_generation,
                        gateway_epoch: current.gateway_epoch,
                    })?,
                )
                .await?;
            ensure!(
                ack["acknowledged"] == true,
                "runtime_acknowledgement_invalid"
            );
            identity.acknowledge()?;
        }
        identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))
    }
    fn validate(&self, bootstrap: &CloudRuntimeBootstrap, binding: &IdentityBinding) -> Result<()> {
        ensure!(
            bootstrap.workspace_id == binding.workspace
                && bootstrap.runtime_generation == binding.generation
                && bootstrap.rpc_protocol.as_deref() == Some(RUST_RPC)
                && bootstrap.gateway_protocol == RUST_PROTOCOL
                && bootstrap
                    .runtime_storage
                    .as_ref()
                    .is_some_and(|s| s.storage_incarnation_id == binding.incarnation
                        && s.format == zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT),
            "runtime_bootstrap_binding_mismatch"
        );
        let mut expected = self.origin.clone();
        expected
            .set_scheme(if self.origin.scheme() == "https" {
                "wss"
            } else {
                "ws"
            })
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?;
        expected
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?
            .clear()
            .extend(["v1", "cloud", "workspaces", &binding.workspace, "gateway"]);
        ensure!(
            bootstrap.gateway_url == expected.as_str(),
            "runtime_gateway_origin_mismatch"
        );
        Ok(())
    }
    pub async fn renew(&self, identity: &mut Identity) -> Result<()> {
        let request = identity.begin_renewal()?;
        let proof = identity.renewal_proof(&request, now_ms())?;
        let boot = identity
            .bootstrap()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        let response = self
            .post(
                &boot.workspace_id,
                &["runtime", "credentials", "renew"],
                &boot.runtime_credential,
                &json!({"requestId":request,"proof":proof}),
            )
            .await?;
        identity.commit_renewal(&response)
    }
    pub async fn prepare_storage(&self, bootstrap: &CloudRuntimeBootstrap) -> Result<String> {
        if bootstrap.extra.get("startupBindingVersion") == Some(&json!(2)) {
            let enrollment = bootstrap
                .extra
                .get("storageEnrollmentId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.len() <= 512)
                .ok_or_else(|| anyhow::anyhow!("runtime_storage_enrollment_rejected"))?;
            return Ok(enrollment.to_owned());
        }
        self.storage(bootstrap, None).await
    }
    pub async fn commit_storage(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        enrollment: &str,
    ) -> Result<()> {
        if bootstrap.extra.get("startupBindingVersion") == Some(&json!(2)) {
            ensure!(
                bootstrap
                    .extra
                    .get("storageEnrollmentId")
                    .and_then(Value::as_str)
                    == Some(enrollment),
                "runtime_storage_enrollment_rejected"
            );
            // The caller committed local storage. The first authenticated gateway
            // bind durably commits the same enrollment at the authority.
            return Ok(());
        }
        ensure!(
            self.storage(bootstrap, Some(enrollment)).await? == enrollment,
            "runtime_storage_enrollment_rejected"
        );
        Ok(())
    }
    async fn storage(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        enrollment: Option<&str>,
    ) -> Result<String> {
        let body = Self::storage_request(bootstrap, enrollment)?;
        let response = self
            .post(
                &bootstrap.workspace_id,
                &["runtime", "storage", "enroll"],
                &bootstrap.runtime_credential,
                &serde_json::to_value(body)?,
            )
            .await?;
        response["enrollmentId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("runtime_storage_enrollment_rejected"))
    }
    pub async fn launch_ready(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        command: &str,
        head: i64,
    ) -> Result<()> {
        self.post(
            &bootstrap.workspace_id,
            &["ready"],
            &bootstrap.runtime_credential,
            &json!({"phase":"agent-started","launchCommandId":command,"sessionHeadVersion":head}),
        )
        .await?;
        Ok(())
    }
    fn storage_request(
        bootstrap: &CloudRuntimeBootstrap,
        enrollment: Option<&str>,
    ) -> Result<zuse_protocol::cloud::CloudStorageEnrollment> {
        use zuse_protocol::cloud::{CloudStorageEnrollment, CloudStoragePhase};
        Ok(CloudStorageEnrollment {
            phase: if enrollment.is_some() {
                CloudStoragePhase::Commit
            } else {
                CloudStoragePhase::Prepare
            },
            runtime_generation: bootstrap.runtime_generation,
            gateway_epoch: bootstrap.gateway_epoch,
            storage_incarnation_id: bootstrap
                .runtime_storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("runtime_storage_enrollment_required"))?
                .storage_incarnation_id
                .clone(),
            enrollment_id: enrollment.map(str::to_owned),
            capabilities: enrollment.map(|_| {
                let mut capabilities = vec![
                    "rust-rpc-v1".into(),
                    "rust-session-commands-v1".into(),
                    zuse_protocol::cloud_lifecycle::CAPABILITY.into(),
                    zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY.into(),
                    "rust-transcript-projection-v1".into(),
                    "rust-full-state-recovery-v1".into(),
                ];
                if bootstrap.extra.get("commandDeliveryMode")
                    == Some(&json!(zuse_protocol::cloud_task_commands::MODE))
                    && bootstrap.extra.get("commandAdmissionVersion")
                        == Some(&json!(zuse_protocol::cloud_task_commands::VERSION))
                {
                    capabilities.push(zuse_protocol::cloud_task_commands::CAPABILITY.into());
                }
                if bootstrap
                    .extra
                    .get("rustProjectCache")
                    .is_some_and(|v| !v.is_null())
                {
                    capabilities.push(crate::project_cache::CAPABILITY.into());
                }
                if bootstrap.extra.get("rustRecreationVersion") == Some(&json!(1)) {
                    capabilities.push(zuse_protocol::cloud_lifecycle::RECREATION_CAPABILITY.into());
                }
                if bootstrap.extra.get("rustOrganizationExecutionVersion") == Some(&json!(1)) {
                    capabilities.push("rust-organization-execution-v1".into());
                }
                capabilities
            }),
        })
    }
    pub async fn early_agent_ready(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        command: &str,
    ) -> Result<()> {
        self.post(
            &bootstrap.workspace_id,
            &["ready"],
            &bootstrap.runtime_credential,
            &json!({"phase":"agent-ready","launchCommandId":command,"sessionHeadVersion":0}),
        )
        .await?;
        Ok(())
    }
    pub async fn preparation_ingress(&self, bootstrap: &CloudRuntimeBootstrap) -> Result<Value> {
        self.post(
            &bootstrap.workspace_id,
            &["runtime", "preparation", "ingress"],
            &bootstrap.runtime_credential,
            &json!({"operationId":bootstrap.extra.get("rustAllocationOperationId").cloned().unwrap_or_default()}),
        )
        .await
    }
    pub async fn preparation(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        update: &zuse_protocol::cloud_computers::RuntimePreparationUpdate,
    ) -> Result<()> {
        self.post(
            &bootstrap.workspace_id,
            &["runtime", "preparation"],
            &bootstrap.runtime_credential,
            &serde_json::to_value(update)?,
        )
        .await?;
        Ok(())
    }
    /// `GET /v1/cloud/workspaces/{id}/runtime/update`.
    pub async fn runtime_update(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
    ) -> Result<zuse_protocol::cloud_runtime_updates::UpdateOffer> {
        let value = self
            .get(
                &bootstrap.workspace_id,
                &["runtime", "update"],
                &bootstrap.runtime_credential,
            )
            .await?;
        serde_json::from_value(value).map_err(|_| anyhow::anyhow!("runtime_update_offer_invalid"))
    }
    /// `POST /v1/cloud/workspaces/{id}/runtime/update/attestation`.
    pub async fn runtime_update_attestation(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        attestation: &zuse_protocol::cloud_runtime_updates::RuntimeAttestation,
    ) -> Result<()> {
        self.post(
            &bootstrap.workspace_id,
            &["runtime", "update", "attestation"],
            &bootstrap.runtime_credential,
            &serde_json::to_value(attestation)?,
        )
        .await?;
        Ok(())
    }
    /// `POST /v1/cloud/workspaces/{id}/runtime/harness-updates`.
    pub async fn harness_updates(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        poll: &zuse_protocol::cloud_harness_updates::HarnessPoll,
    ) -> Result<zuse_protocol::cloud_harness_updates::HarnessOffer> {
        let value = self
            .post(
                &bootstrap.workspace_id,
                &["runtime", "harness-updates"],
                &bootstrap.runtime_credential,
                &serde_json::to_value(poll)?,
            )
            .await?;
        serde_json::from_value(value).map_err(|_| anyhow::anyhow!("harness_update_offer_invalid"))
    }
    /// `POST /v1/cloud/workspaces/{id}/runtime/harness-updates/attestation`.
    pub async fn harness_update_attestation(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        attestation: &zuse_protocol::cloud_harness_updates::HarnessAttestation,
    ) -> Result<()> {
        self.post(
            &bootstrap.workspace_id,
            &["runtime", "harness-updates", "attestation"],
            &bootstrap.runtime_credential,
            &serde_json::to_value(attestation)?,
        )
        .await?;
        Ok(())
    }
    pub fn gateway(&self, bootstrap: &CloudRuntimeBootstrap) -> Result<zuse_rpc::cloud::Grant> {
        Ok(zuse_rpc::cloud::Grant {
            workspace_id: bootstrap.workspace_id.clone(),
            ws_url: bootstrap.gateway_url.clone(),
            protocol: bootstrap.gateway_protocol.clone(),
            rpc_protocol: bootstrap.rpc_protocol.clone().unwrap_or_default(),
            credential: bootstrap.runtime_credential.clone(),
            generation: bootstrap.runtime_generation,
            gateway_epoch: bootstrap.gateway_epoch,
            expires_at: bootstrap.runtime_credential_expires_at,
            storage_binding: if bootstrap.extra.get("startupBindingVersion") == Some(&json!(2)) {
                let id = bootstrap
                    .extra
                    .get("storageEnrollmentId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("runtime_storage_enrollment_rejected"))?;
                Some(json!(Self::storage_request(bootstrap, Some(id))?))
            } else {
                None
            },
        })
    }
}
