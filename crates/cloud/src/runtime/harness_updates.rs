//! Offering agent CLI harness bundles to computers and recording what they
//! adopted (`infra/cloud/HARNESS_BUNDLES.md`).
//!
//! ```text
//! runtime ── POST …/runtime/harness-updates ─────────────▶ record each agent's
//!            HarnessPoll { architecture, agents }          current binding, then
//!                                                          offer one bundle per agent
//! runtime ── POST …/runtime/harness-updates/attestation ─▶ adopted: binding + offer
//!                                                          failed: count a failure
//! ```
//! The registry is operator-written (`crate::harness_bundles`); this module
//! only reads it and keeps per-computer offers, bindings and failures. The
//! workspace row is read for authorization only and never locked for
//! update, so a harness poll never waits on or blocks command admission.
use super::*;
use crate::harness_bundles::{self as registry, Bundle, Channel, Current};
use std::collections::BTreeSet;
use zuse_protocol::cloud_harness_updates as wire;

/// Which harness bundles this deployment offers.
#[derive(Clone, Default)]
pub struct HarnessBundleConfig {
    pub channel: Channel,
}
impl HarnessBundleConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            channel: Channel::from_env()?,
        })
    }
}

fn short_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}
fn binding_valid(binding: &wire::HarnessBinding) -> bool {
    binding.sha256.as_deref().is_none_or(wire::sha256_valid)
        && !binding.version.is_empty()
        && binding.version.len() <= 64
        && wire::name_valid(&binding.state_format)
}
fn poll_valid(poll: &wire::HarnessPoll) -> bool {
    let mut seen = BTreeSet::new();
    matches!(poll.architecture.as_str(), "x86_64" | "aarch64")
        && poll.agents.len() <= wire::AGENTS.len()
        && poll.agents.iter().all(|a| {
            wire::AGENTS.contains(&a.agent.as_str())
                && seen.insert(a.agent.clone())
                && !a.adapters.is_empty()
                && a.adapters.len() <= 8
                && a.adapters.iter().all(|n| wire::name_valid(n))
                && a.current.as_ref().is_none_or(binding_valid)
        })
}

impl RuntimeControl {
    pub fn with_harness_bundles(mut self, config: HarnessBundleConfig) -> Self {
        self.harness_bundles = config;
        self
    }

    /// Authorize the runtime credential against an unlocked workspace read.
    async fn harness_runtime_row(
        &self,
        id: &str,
        credential: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.runtime_scope_access(id, credential, now).await?;
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let row = row.ok_or_else(rejected)?;
        mailbox::runtime_auth(&row, credential, now)?;
        if blocked(&row) || row["desired_state"] != "ready" {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        Ok(row)
    }

    /// `POST /v1/cloud/workspaces/{id}/runtime/harness-updates`.
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="harness_update_poll"))]
    pub async fn harness_update_poll(
        &self,
        id: &str,
        credential: &str,
        poll: wire::HarnessPoll,
        now: i64,
    ) -> Result<wire::HarnessOffer, ApiError> {
        if !poll_valid(&poll) {
            return Err(ApiError::bad_request("invalid_harness_poll"));
        }
        let row = self.harness_runtime_row(id, credential, now).await?;
        if !runtime_capable(&row, wire::CAPABILITY) {
            return Ok(wire::HarnessOffer::default());
        }
        let states = self
            .harness_bundles
            .channel
            .states(row["account_id"].as_str());
        let mut tx = self.pool.begin().await?;
        let mut targets = Vec::new();
        for agent in &poll.agents {
            if let Some(binding) = &agent.current {
                sqlx::query("INSERT INTO zuse_rust_harness_bindings(workspace_id,agent,sha256,version,state_format,reported_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(workspace_id,agent) DO UPDATE SET sha256=EXCLUDED.sha256,version=EXCLUDED.version,state_format=EXCLUDED.state_format,previous_sha256=CASE WHEN zuse_rust_harness_bindings.sha256 IS NOT DISTINCT FROM EXCLUDED.sha256 THEN zuse_rust_harness_bindings.previous_sha256 ELSE zuse_rust_harness_bindings.sha256 END,adopted_at=CASE WHEN zuse_rust_harness_bindings.sha256 IS NOT DISTINCT FROM EXCLUDED.sha256 THEN zuse_rust_harness_bindings.adopted_at END,reported_at=EXCLUDED.reported_at")
                    .bind(id)
                    .bind(&agent.agent)
                    .bind(&binding.sha256)
                    .bind(&binding.version)
                    .bind(&binding.state_format)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
            let rows: Vec<registry::BundleRow> = sqlx::query_as(&format!(
                "SELECT {} FROM zuse_rust_harness_bundles WHERE agent=$1 AND architecture=$2 AND (state=ANY($3) OR sha256=$4)",
                registry::BUNDLE_COLUMNS
            ))
            .bind(&agent.agent)
            .bind(&poll.architecture)
            .bind(&states)
            .bind(agent.current.as_ref().and_then(|c| c.sha256.as_deref()))
            .fetch_all(&mut *tx)
            .await?;
            let (mut current_row, mut candidates) = (None, Vec::new());
            for bundle in rows.into_iter().map(Bundle::from_row) {
                let is_current = agent.current.as_ref().and_then(|c| c.sha256.as_ref())
                    == Some(&bundle.signed.sha256);
                if is_current {
                    current_row = Some(bundle.clone());
                }
                if states.contains(&bundle.state.as_str()) {
                    candidates.push(bundle);
                }
            }
            let failed: BTreeSet<String> = sqlx::query_scalar("SELECT sha256 FROM zuse_rust_harness_update_failures WHERE workspace_id=$1 AND failures>=$2")
                .bind(id)
                .bind(registry::MAX_FAILURES)
                .fetch_all(&mut *tx)
                .await?
                .into_iter()
                .collect();
            let current = agent.current.as_ref().map(|binding| Current {
                binding,
                registered: current_row.as_ref(),
            });
            if let Some(target) =
                registry::pick_target(current, &agent.adapters, candidates, &failed)
            {
                targets.push(target);
            }
        }
        let mut directives = Vec::new();
        if !targets.is_empty() {
            let links =
                self.runtime_releases.links.as_ref().ok_or_else(|| {
                    ApiError::unavailable("cloud_runtime_release_store_unavailable")
                })?;
            for target in targets {
                let (url, url_expires_at) = links.get_url(&target.artifact_key, now)?;
                let prior: Option<(String, String, String)> = sqlx::query_as("SELECT update_id,target_sha256,state FROM zuse_rust_harness_updates WHERE workspace_id=$1 AND agent=$2 FOR UPDATE")
                    .bind(id)
                    .bind(&target.signed.agent)
                    .fetch_optional(&mut *tx)
                    .await?;
                // The same in-flight offer keeps its id; anything else starts a new attempt.
                let update_id = match prior {
                    Some((update_id, sha, state))
                        if sha == target.signed.sha256 && state == "offered" =>
                    {
                        update_id
                    }
                    _ => uuid::Uuid::new_v4().to_string(),
                };
                sqlx::query("INSERT INTO zuse_rust_harness_updates(workspace_id,agent,update_id,target_sha256,state,error_code,offered_at,created_at,updated_at) VALUES($1,$2,$3,$4,'offered',NULL,$5,$5,$5) ON CONFLICT(workspace_id,agent) DO UPDATE SET update_id=EXCLUDED.update_id,target_sha256=EXCLUDED.target_sha256,state='offered',error_code=NULL,offered_at=EXCLUDED.offered_at,attested_at=NULL,updated_at=EXCLUDED.updated_at")
                    .bind(id)
                    .bind(&target.signed.agent)
                    .bind(&update_id)
                    .bind(&target.signed.sha256)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                directives.push(wire::HarnessDirective {
                    update_id,
                    artifact: wire::HarnessArtifact {
                        compressed_size_bytes: u64::try_from(target.compressed_size_bytes)
                            .map_err(|_| ApiError::unavailable("harness_bundle_invalid"))?,
                        bundle: target.signed,
                        signature: target.signature,
                        url,
                        url_expires_at,
                    },
                });
            }
        }
        tx.commit().await?;
        Ok(wire::HarnessOffer { directives })
    }

    /// `POST /v1/cloud/workspaces/{id}/runtime/harness-updates/attestation`.
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="harness_update_attestation"))]
    pub async fn harness_update_attest(
        &self,
        id: &str,
        credential: &str,
        attestation: wire::HarnessAttestation,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !wire::AGENTS.contains(&attestation.agent.as_str())
            || !wire::sha256_valid(&attestation.sha256)
            || !wire::version_valid(&attestation.version)
            || attestation
                .previous_sha256
                .as_deref()
                .is_some_and(|s| !wire::sha256_valid(s))
            || attestation
                .error_code
                .as_deref()
                .is_some_and(|c| !short_code(c))
            || attestation.update_id.as_deref().is_some_and(|u| {
                u.is_empty()
                    || u.len() > 128
                    || !u
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err(ApiError::bad_request("invalid_harness_attestation"));
        }
        self.harness_runtime_row(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let prior: Option<(String, String, String)> = sqlx::query_as("SELECT update_id,target_sha256,state FROM zuse_rust_harness_updates WHERE workspace_id=$1 AND agent=$2 FOR UPDATE")
            .bind(id)
            .bind(&attestation.agent)
            .fetch_optional(&mut *tx)
            .await?;
        // Only the attempt in flight counts; a duplicate or stale report changes nothing.
        let current_attempt = prior.as_ref().filter(|(update_id, sha, state)| {
            state == "offered"
                && *sha == attestation.sha256
                && attestation
                    .update_id
                    .as_deref()
                    .is_none_or(|u| u == update_id)
        });
        match attestation.outcome {
            wire::HarnessOutcome::Adopted => {
                let state_format: String = sqlx::query_scalar(
                    "SELECT state_format FROM zuse_rust_harness_bundles WHERE sha256=$1 AND agent=$2",
                )
                .bind(&attestation.sha256)
                .bind(&attestation.agent)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| ApiError::conflict("harness_bundle_unknown"))?;
                sqlx::query("INSERT INTO zuse_rust_harness_bindings(workspace_id,agent,sha256,version,state_format,previous_sha256,adopted_at,reported_at) VALUES($1,$2,$3,$4,$5,$6,$7,$7) ON CONFLICT(workspace_id,agent) DO UPDATE SET previous_sha256=CASE WHEN zuse_rust_harness_bindings.sha256 IS NOT DISTINCT FROM EXCLUDED.sha256 THEN zuse_rust_harness_bindings.previous_sha256 ELSE COALESCE(EXCLUDED.previous_sha256,zuse_rust_harness_bindings.sha256) END,adopted_at=CASE WHEN zuse_rust_harness_bindings.sha256 IS NOT DISTINCT FROM EXCLUDED.sha256 THEN COALESCE(zuse_rust_harness_bindings.adopted_at,EXCLUDED.adopted_at) ELSE EXCLUDED.adopted_at END,sha256=EXCLUDED.sha256,version=EXCLUDED.version,state_format=EXCLUDED.state_format,reported_at=EXCLUDED.reported_at")
                    .bind(id)
                    .bind(&attestation.agent)
                    .bind(&attestation.sha256)
                    .bind(&attestation.version)
                    .bind(&state_format)
                    .bind(&attestation.previous_sha256)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                if current_attempt.is_some() {
                    sqlx::query("UPDATE zuse_rust_harness_updates SET state='adopted',error_code=NULL,attested_at=$3,updated_at=$3 WHERE workspace_id=$1 AND agent=$2")
                        .bind(id)
                        .bind(&attestation.agent)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            wire::HarnessOutcome::Failed => {
                if current_attempt.is_some() {
                    let code = attestation
                        .error_code
                        .clone()
                        .unwrap_or_else(|| "harness_update_failed".into());
                    sqlx::query("INSERT INTO zuse_rust_harness_update_failures(workspace_id,sha256,failures,error_code,updated_at) VALUES($1,$2,1,$3,$4) ON CONFLICT(workspace_id,sha256) DO UPDATE SET failures=zuse_rust_harness_update_failures.failures+1,error_code=EXCLUDED.error_code,updated_at=EXCLUDED.updated_at")
                        .bind(id)
                        .bind(&attestation.sha256)
                        .bind(&code)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("UPDATE zuse_rust_harness_updates SET state='failed',error_code=$3,attested_at=$4,updated_at=$4 WHERE workspace_id=$1 AND agent=$2")
                        .bind(id)
                        .bind(&attestation.agent)
                        .bind(&code)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(json!({"recorded": true}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn agent(name: &str) -> wire::AgentHarness {
        wire::AgentHarness {
            agent: name.into(),
            adapters: vec![wire::adapter_for(name).unwrap_or("x").into()],
            current: Some(wire::HarnessBinding {
                sha256: None,
                version: "1.0.0".into(),
                state_format: "image-tested-v1".into(),
            }),
        }
    }
    #[test]
    fn polls_are_bounded_and_well_formed() {
        let poll = |agents| wire::HarnessPoll {
            architecture: "x86_64".into(),
            agents,
        };
        assert!(poll_valid(&poll(vec![agent("claude"), agent("codex")])));
        assert!(!poll_valid(&poll(vec![agent("claude"), agent("claude")])));
        assert!(!poll_valid(&poll(vec![agent("mock")])));
        let mut bad = agent("grok");
        bad.current.as_mut().unwrap().sha256 = Some("Z".repeat(64));
        assert!(!poll_valid(&poll(vec![bad])));
        let mut none = agent("grok");
        none.adapters.clear();
        assert!(!poll_valid(&poll(vec![none])));
        let mut arch = poll(vec![]);
        arch.architecture = "riscv".into();
        assert!(!poll_valid(&arch));
    }
}
