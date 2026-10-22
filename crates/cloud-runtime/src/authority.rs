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
    pub fn origin(&self) -> String {
        self.origin.origin().ascii_serialization()
    }
    async fn post(
        &self,
        workspace: &str,
        suffix: &[&str],
        credential: &str,
        body: &Value,
    ) -> Result<Value> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("runtime_api_origin_invalid"))?
            .clear()
            .extend(["v1", "cloud", "workspaces", workspace])
            .extend(suffix.iter().copied());
        let mut response = self
            .client
            .post(url)
            .bearer_auth(credential)
            .json(body)
            .send()
            .await
            .map_err(|_| Failure {
                code: "runtime_authority_unreachable",
                retryable: true,
            })?;
        let status = response.status();
        ensure!(
            response
                .content_length()
                .is_none_or(|l| l <= 2 * 1024 * 1024),
            "runtime_authority_response_invalid"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure {
            code: "runtime_authority_unreachable",
            retryable: true,
        })? {
            ensure!(
                bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                "runtime_authority_response_invalid"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            const CODES: &[&str] = &[
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
        serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("runtime_authority_response_invalid"))
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
        if identity.has_pending_renewal()
            || current.runtime_credential_expires_at <= now_ms() + 60000
        {
            self.renew(identity).await?;
        }
        if !identity.acknowledged() {
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
        self.storage(bootstrap, None).await
    }
    pub async fn commit_storage(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
        enrollment: &str,
    ) -> Result<()> {
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
        use zuse_protocol::cloud::{CloudStorageEnrollment, CloudStoragePhase};
        let body = CloudStorageEnrollment {
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
        };
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
    pub fn gateway(&self, bootstrap: &CloudRuntimeBootstrap) -> zuse_rpc::cloud::Grant {
        zuse_rpc::cloud::Grant {
            workspace_id: bootstrap.workspace_id.clone(),
            ws_url: bootstrap.gateway_url.clone(),
            protocol: bootstrap.gateway_protocol.clone(),
            rpc_protocol: bootstrap.rpc_protocol.clone().unwrap_or_default(),
            credential: bootstrap.runtime_credential.clone(),
            generation: bootstrap.runtime_generation,
            gateway_epoch: bootstrap.gateway_epoch,
            expires_at: bootstrap.runtime_credential_expires_at,
        }
    }
}
