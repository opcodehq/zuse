//! Moving a computer to a newer guest runtime in place
//! (`internal-docs/cloud-runtime-updates.md`).
//!
//! ```text
//! runtime ── GET  …/runtime/update ─────────────▶ offer: target release + presigned URL
//! runtime ── POST …/runtime/update/attestation ─▶ committed: pin runtimeRelease and
//!                                                 replace stored capabilities
//!                                                 rolled-back/failed: count a failure
//! admission: message needs a missing capability ─▶ urgency now + RUNTIME_UPDATING,
//!                                                 or RUNTIME_UPDATE_UNAVAILABLE
//! app ────── GET  …/runtime-update ─────────────▶ UpdateStatus
//! ```
//! The registry is operator-written (`crate::runtime_releases`); this module
//! only reads it and keeps the per-computer offer in `zuse_rust_runtime_updates`.
use super::*;
use crate::runtime_releases::{self as registry, Channel, Release, Version};
use sqlx::PgConnection;
use zuse_protocol::cloud_runtime_updates as wire;

/// After this many failed attempts a release is not offered to that computer.
const MAX_FAILURES: i32 = 2;

/// Which channel this deployment follows and where artifacts are downloaded.
#[derive(Clone)]
pub struct RuntimeReleaseConfig {
    pub channel: Channel,
    pub links: Option<Arc<crate::objects::ArtifactLinks>>,
}
impl Default for RuntimeReleaseConfig {
    fn default() -> Self {
        Self {
            channel: Channel::Stable,
            links: None,
        }
    }
}
impl RuntimeReleaseConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            channel: Channel::from_env()?,
            links: crate::objects::ArtifactLinks::from_env()?.map(Arc::new),
        })
    }
}

struct Target {
    release: Release,
    failures: i32,
    error_code: Option<String>,
}
impl Target {
    fn exhausted(&self) -> bool {
        self.failures >= MAX_FAILURES
    }
}

/// The offer the server last made this computer.
struct Offer {
    update_id: String,
    target_sha256: String,
    urgency: String,
    state: String,
    error_code: Option<String>,
}
async fn offer(conn: &mut PgConnection, id: &str, lock: bool) -> Result<Option<Offer>, ApiError> {
    let statement = if lock {
        "SELECT update_id,target_sha256,urgency,state,error_code FROM zuse_rust_runtime_updates WHERE workspace_id=$1 FOR UPDATE"
    } else {
        "SELECT update_id,target_sha256,urgency,state,error_code FROM zuse_rust_runtime_updates WHERE workspace_id=$1"
    };
    let row: Option<(String, String, String, String, Option<String>)> = sqlx::query_as(statement)
        .bind(id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(
        |(update_id, target_sha256, urgency, state, error_code)| Offer {
            update_id,
            target_sha256,
            urgency,
            state,
            error_code,
        },
    ))
}
fn in_flight(offer: &Offer, sha: &str) -> bool {
    offer.target_sha256 == sha && matches!(offer.state.as_str(), "pending" | "downloading")
}

/// The runtime this computer serves: its last committed update, else the
/// base's pinned runtime.
fn current_sha(config: &Value) -> Option<&str> {
    config["runtimeRelease"]["sha256"]
        .as_str()
        .or_else(|| config["initialRuntimeBootstrap"]["runtimeSha256"].as_str())
        .filter(|s| registry::sha256_valid(s))
}

/// A fresh machine (allocation, restore, recreation, unarchive) boots the
/// base image's runtime; only a retained resume keeps an updated one. Drop
/// the pin so target selection judges the computer by its base runtime and
/// offers the update again.
pub(super) fn fresh_machine(config: &mut Value) {
    if let Some(config) = config.as_object_mut() {
        config.remove("runtimeRelease");
    }
}

fn short_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
}

impl RuntimeControl {
    pub fn with_runtime_releases(mut self, config: RuntimeReleaseConfig) -> Self {
        self.runtime_releases = config;
        self
    }

    /// Newest release in this deployment's channel for the computer's
    /// architecture that is newer than what it runs now.
    async fn runtime_target(
        &self,
        conn: &mut PgConnection,
        row: &Value,
    ) -> Result<Option<Target>, ApiError> {
        let id = required(row, "workspace_id")?;
        let config = &row["request_config"];
        let base: Option<(String, Option<String>)> = match (
            row["provider"].as_str(),
            row["managed_release_id"].as_str(),
        ) {
            (Some(provider), Some(release)) => sqlx::query_as("SELECT architecture,runtime_manifest#>>'{manifest,commandAdmission,runtimeSha256}' FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2")
                .bind(provider)
                .bind(release)
                .fetch_optional(&mut *conn)
                .await?,
            _ => None,
        };
        let current = current_sha(config)
            .map(str::to_owned)
            .or_else(|| base.as_ref().and_then(|(_, sha)| sha.clone()))
            .filter(|s| registry::sha256_valid(s));
        let registered: Option<(String, String, i64)> = match &current {
            Some(sha) => sqlx::query_as(
                "SELECT version,architecture,created_at FROM zuse_rust_runtime_releases WHERE sha256=$1",
            )
            .bind(sha)
            .fetch_optional(&mut *conn)
            .await?,
            None => None,
        };
        let Some(architecture) = registered
            .as_ref()
            .map(|(_, arch, _)| arch.clone())
            .or_else(|| base.map(|(arch, _)| arch))
        else {
            return Ok(None);
        };
        let rows: Vec<registry::ReleaseRow> = sqlx::query_as(&format!(
            "SELECT {} FROM zuse_rust_runtime_releases WHERE architecture=$1 AND state=ANY($2)",
            registry::RELEASE_COLUMNS
        ))
        .bind(&architecture)
        .bind(self.runtime_releases.channel.states())
        .fetch_all(&mut *conn)
        .await?;
        let candidates = rows
            .into_iter()
            .map(Release::from_row)
            .filter(|r| current.as_deref() != Some(r.sha256.as_str()))
            .collect();
        let version = registered
            .as_ref()
            .and_then(|(version, _, at)| Version::parse(version).map(|v| (v, *at)));
        let pinned = match (&current, &version) {
            (Some(sha), Some((version, at))) => Some((sha.as_str(), version, *at)),
            _ => None,
        };
        let Some(release) = registry::pick_target(pinned, candidates) else {
            return Ok(None);
        };
        let failures: Option<(i32, Option<String>)> = sqlx::query_as("SELECT failures,error_code FROM zuse_rust_runtime_update_failures WHERE workspace_id=$1 AND sha256=$2")
            .bind(id)
            .bind(&release.sha256)
            .fetch_optional(&mut *conn)
            .await?;
        let (failures, error_code) = failures.unwrap_or((0, None));
        Ok(Some(Target {
            release,
            failures,
            error_code,
        }))
    }

    /// `GET /v1/cloud/workspaces/{id}/runtime/update` for the computer's runtime.
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="runtime_update_offer"))]
    pub async fn runtime_update_offer(
        &self,
        id: &str,
        credential: &str,
        now: i64,
    ) -> Result<wire::UpdateOffer, ApiError> {
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Command).await?;
        mailbox::runtime_auth(&row, credential, now)?;
        if !runtime_capable(&row, wire::CAPABILITY) {
            return Ok(wire::UpdateOffer { directive: None });
        }
        let target = match self.runtime_target(&mut tx, &row).await? {
            Some(target) if !target.exhausted() => target,
            _ => return Ok(wire::UpdateOffer { directive: None }),
        };
        let links = self
            .runtime_releases
            .links
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_runtime_release_store_unavailable"))?;
        let (url, url_expires_at) = links.get_url(&target.release.artifact_key, now)?;
        let sha = &target.release.sha256;
        let prior = offer(&mut tx, id, true).await?;
        // The same in-flight attempt keeps its id and urgency; anything else
        // (a new target, or a retry after a failure) starts a new attempt.
        let (update_id, urgency) = match prior.as_ref().filter(|o| in_flight(o, sha)) {
            Some(o) => (o.update_id.clone(), o.urgency.clone()),
            None => (uuid::Uuid::new_v4().to_string(), "background".to_owned()),
        };
        sqlx::query("INSERT INTO zuse_rust_runtime_updates(workspace_id,update_id,target_sha256,urgency,state,error_code,offered_at,created_at,updated_at) VALUES($1,$2,$3,$4,'downloading',NULL,$5,$5,$5) ON CONFLICT(workspace_id) DO UPDATE SET update_id=EXCLUDED.update_id,target_sha256=EXCLUDED.target_sha256,urgency=EXCLUDED.urgency,state='downloading',error_code=CASE WHEN zuse_rust_runtime_updates.update_id=EXCLUDED.update_id THEN zuse_rust_runtime_updates.error_code END,offered_at=EXCLUDED.offered_at,updated_at=EXCLUDED.updated_at")
            .bind(id)
            .bind(&update_id)
            .bind(sha)
            .bind(&urgency)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let compressed_size_bytes = u64::try_from(target.release.compressed_size_bytes)
            .map_err(|_| ApiError::unavailable("cloud_runtime_release_invalid"))?;
        Ok(wire::UpdateOffer {
            directive: Some(wire::UpdateDirective {
                update_id,
                urgency: if urgency == "now" {
                    wire::Urgency::Now
                } else {
                    wire::Urgency::Background
                },
                artifact: wire::RuntimeArtifact {
                    release: target.release.signed(),
                    signature: target.release.signature.clone(),
                    url,
                    compressed_size_bytes,
                    url_expires_at,
                },
            }),
        })
    }

    /// `POST /v1/cloud/workspaces/{id}/runtime/update/attestation`.
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(workspace=%id, operation="runtime_update_attestation"))]
    pub async fn runtime_update_attest(
        &self,
        id: &str,
        credential: &str,
        attestation: wire::RuntimeAttestation,
        now: i64,
    ) -> Result<Value, ApiError> {
        let invalid = || ApiError::bad_request("invalid_runtime_attestation");
        if !registry::sha256_valid(&attestation.sha256)
            || attestation.version.is_empty()
            || attestation.version.len() > 64
            || !registry::names_valid(&attestation.capabilities)
            || attestation
                .previous_sha256
                .as_deref()
                .is_some_and(|s| !registry::sha256_valid(s))
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
            return Err(invalid());
        }
        self.runtime_scope_access(id, credential, now).await?;
        let mut tx = self.pool.begin().await?;
        // Same exclusive workspace fence as bootstrap, which also writes the
        // stored capabilities.
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        mailbox::runtime_auth(&row, credential, now)?;
        if blocked(&row) || row["desired_state"] != "ready" {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        let prior = offer(&mut tx, id, true).await?;
        match attestation.outcome {
            wire::Outcome::Committed => {
                let version: String = sqlx::query_scalar(
                    "SELECT version FROM zuse_rust_runtime_releases WHERE sha256=$1",
                )
                .bind(&attestation.sha256)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| ApiError::conflict("cloud_runtime_release_unknown"))?;
                let mut config = row["request_config"].clone();
                let current = current_sha(&config).map(str::to_owned);
                let pinned = config["runtimeRelease"].clone();
                let release = if pinned["sha256"] == attestation.sha256.as_str() {
                    // A repeated attestation keeps the original record.
                    pinned
                } else {
                    json!({
                        "sha256": attestation.sha256,
                        "version": version,
                        "previousSha256": current.or(attestation.previous_sha256.clone()),
                        "appliedAt": now,
                    })
                };
                config["runtimeRelease"] = release;
                if config["runtimeBootstrapReceipt"].is_object() {
                    config["runtimeBootstrapReceipt"]["capabilities"] =
                        json!(attestation.capabilities);
                }
                if config["rustStorageEnrollment"]["capabilities"].is_array() {
                    config["rustStorageEnrollment"]["capabilities"] =
                        json!(attestation.capabilities);
                }
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
                    .bind(id)
                    .bind(&config)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                let update_id = prior
                    .as_ref()
                    .map(|o| o.update_id.clone())
                    .or(attestation.update_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                sqlx::query("INSERT INTO zuse_rust_runtime_updates(workspace_id,update_id,target_sha256,urgency,state,attested_at,created_at,updated_at) VALUES($1,$2,$3,'background','committed',$4,$4,$4) ON CONFLICT(workspace_id) DO UPDATE SET target_sha256=EXCLUDED.target_sha256,urgency='background',state='committed',error_code=NULL,attested_at=EXCLUDED.attested_at,updated_at=EXCLUDED.updated_at")
                    .bind(id)
                    .bind(update_id)
                    .bind(&attestation.sha256)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
            outcome @ (wire::Outcome::RolledBack | wire::Outcome::Failed) => {
                let (state, default_code) = match outcome {
                    wire::Outcome::RolledBack => ("rolled-back", "runtime_update_rolled_back"),
                    _ => ("failed", "runtime_update_failed"),
                };
                let code = attestation
                    .error_code
                    .clone()
                    .unwrap_or_else(|| default_code.to_owned());
                // Only the attempt in flight counts; a duplicate or stale
                // report changes nothing.
                if let Some(offer) = prior.filter(|o| {
                    matches!(o.state.as_str(), "pending" | "downloading")
                        && attestation
                            .update_id
                            .as_deref()
                            .is_none_or(|u| u == o.update_id)
                }) {
                    sqlx::query("INSERT INTO zuse_rust_runtime_update_failures(workspace_id,sha256,failures,error_code,updated_at) VALUES($1,$2,1,$3,$4) ON CONFLICT(workspace_id,sha256) DO UPDATE SET failures=zuse_rust_runtime_update_failures.failures+1,error_code=EXCLUDED.error_code,updated_at=EXCLUDED.updated_at")
                        .bind(id)
                        .bind(&offer.target_sha256)
                        .bind(&code)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("UPDATE zuse_rust_runtime_updates SET state=$2,error_code=$3,urgency='background',attested_at=$4,updated_at=$4 WHERE workspace_id=$1")
                        .bind(id)
                        .bind(state)
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

    /// `GET /v1/cloud/workspaces/{id}/runtime-update` for an authorized user.
    pub async fn runtime_update_status(&self, row: &Value) -> Result<wire::UpdateStatus, ApiError> {
        let status = |state, update_id, error_code| wire::UpdateStatus {
            update_id,
            state,
            error_code,
        };
        if !runtime_capable(row, wire::CAPABILITY) {
            return Ok(status(
                wire::UpdateState::Unavailable,
                None,
                Some("runtime_update_unsupported".into()),
            ));
        }
        let id = required(row, "workspace_id")?;
        let mut conn = self.pool.acquire().await?;
        let prior = offer(&mut conn, id, false).await?;
        let Some(target) = self.runtime_target(&mut conn, row).await? else {
            return Ok(match prior {
                Some(o)
                    if o.state == "committed"
                        && current_sha(&row["request_config"]) == Some(&o.target_sha256) =>
                {
                    status(wire::UpdateState::Committed, Some(o.update_id), None)
                }
                _ => status(wire::UpdateState::Current, None, None),
            });
        };
        if target.exhausted() {
            return Ok(status(
                wire::UpdateState::Unavailable,
                prior.map(|o| o.update_id),
                Some(
                    target
                        .error_code
                        .unwrap_or_else(|| "runtime_update_retries_exhausted".into()),
                ),
            ));
        }
        Ok(
            match prior.filter(|o| o.target_sha256 == target.release.sha256) {
                Some(o) => {
                    let state = match o.state.as_str() {
                        "downloading" => wire::UpdateState::Downloading,
                        "rolled-back" => wire::UpdateState::RolledBack,
                        "failed" => wire::UpdateState::Failed,
                        _ => wire::UpdateState::Pending,
                    };
                    status(state, Some(o.update_id), o.error_code)
                }
                None => status(wire::UpdateState::Pending, None, None),
            },
        )
    }

    /// A message needs `capability`, which this computer lacks. If a release
    /// that has it can be installed in place, ask the runtime to swap now and
    /// refuse the message with `RUNTIME_UPDATING`; otherwise
    /// `RUNTIME_UPDATE_UNAVAILABLE`. Nothing is enqueued either way.
    ///
    /// There is no server-to-runtime push over the gateway (it relays client
    /// frames only), so the runtime learns the urgency on its next poll.
    pub(in crate::runtime) async fn demand_runtime_update(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        capability: &str,
        now: i64,
    ) -> ApiError {
        let unavailable = || ApiError::conflict(wire::RUNTIME_UPDATE_UNAVAILABLE);
        if !runtime_capable(row, wire::CAPABILITY) {
            return unavailable();
        }
        let target = match self.runtime_target(tx, row).await {
            Ok(Some(target)) if !target.exhausted() && target.release.has(capability) => target,
            Ok(_) => return unavailable(),
            Err(error) => return error,
        };
        let Ok(id) = required(row, "workspace_id") else {
            return unavailable();
        };
        // The admission transaction rolls back with the rejection, so the
        // urgency is written on its own connection. The update table has no
        // reference to the locked workspace row.
        let written = tokio::time::timeout(
            Duration::from_secs(2),
            self.mark_update_now(id, &target.release.sha256, now),
        )
        .await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!(workspace = %id, "Runtime update urgency was not recorded");
        }
        ApiError::conflict(wire::RUNTIME_UPDATING)
    }
    async fn mark_update_now(&self, id: &str, sha: &str, now: i64) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let prior = offer(&mut tx, id, true).await?;
        match prior.filter(|o| in_flight(o, sha)) {
            Some(_) => {
                sqlx::query("UPDATE zuse_rust_runtime_updates SET urgency='now',updated_at=$2 WHERE workspace_id=$1")
                    .bind(id)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
            None => {
                sqlx::query("INSERT INTO zuse_rust_runtime_updates(workspace_id,update_id,target_sha256,urgency,state,created_at,updated_at) VALUES($1,$2,$3,'now','pending',$4,$4) ON CONFLICT(workspace_id) DO UPDATE SET update_id=EXCLUDED.update_id,target_sha256=EXCLUDED.target_sha256,urgency='now',state='pending',error_code=NULL,offered_at=NULL,attested_at=NULL,updated_at=EXCLUDED.updated_at")
                    .bind(id)
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(sha)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_fresh_machine_is_judged_by_its_base_runtime_again() {
        let base = "b".repeat(64);
        let updated = "c".repeat(64);
        let mut config = json!({
            "initialRuntimeBootstrap": {"runtimeSha256": base},
            "runtimeRelease": {"sha256": updated, "version": "1.1.0"},
        });
        assert_eq!(current_sha(&config), Some(updated.as_str()));
        fresh_machine(&mut config);
        assert!(config.get("runtimeRelease").is_none());
        assert_eq!(current_sha(&config), Some(base.as_str()));
    }
}
