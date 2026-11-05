//! Operator-only staging probe. Every mutation uses its own random prefix; no
//! workspace, account, conversation or production object is read or modified.
use super::*;
use serde::Serialize;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub status: &'static str,
    pub prefix: String,
    pub bytes: usize,
    pub cleanup_verified: bool,
    pub error_code: Option<&'static str>,
}
pub async fn staging() -> anyhow::Result<Report> {
    anyhow::ensure!(
        env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").as_deref() == Ok("staging"),
        "Object store probe requires staging"
    );
    let objects = R2::from_env()?
        .ok_or_else(|| anyhow::anyhow!("Staging R2 credentials are not configured"))?;
    let prefix = format!("workspaces/zuse-staging-probe-{}/", uuid::Uuid::new_v4());
    let key = format!("{prefix}roundtrip.bin");
    let bytes = b"zuse-staging-object-check-v1\n".repeat(8192);
    let started = std::time::Instant::now();
    let verification = tokio::time::timeout(Duration::from_secs(90), async {
        objects.put(&key, &bytes).await?;
        if objects.get(&key).await?.as_deref() != Some(bytes.as_slice()) {
            return Err(ApiError::conflict("object_probe_read_mismatch"));
        }
        objects.put(&key, &bytes).await?;
        match objects.put(&key, b"must-never-replace-the-original").await {
            Err(error) if error.code == "cloud_transcript_object_collision" => {}
            _ => return Err(ApiError::conflict("object_probe_immutability_failed")),
        }
        if objects.get(&key).await?.as_deref() != Some(bytes.as_slice()) {
            return Err(ApiError::conflict("object_probe_content_changed"));
        }
        Ok::<(), ApiError>(())
    })
    .await;
    let error = match verification {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.code),
        Err(_) => Some("object_probe_timeout"),
    };
    // A write with an uncertain reply is not proof of absence even after a
    // cleanup attempt. Keep the random prefix in the report for reconciliation.
    let cleanup = tokio::time::timeout(Duration::from_secs(90), async {
        for _ in 0..8 {
            if objects.purge_prefix(&prefix).await? {
                return Ok::<bool, ApiError>(objects.get(&key).await?.is_none());
            }
        }
        Ok(false)
    })
    .await;
    let cleanup_verified = matches!(cleanup, Ok(Ok(true))) && error.is_none();
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        cleanup_verified,
        "staging_object_store_probe_finished"
    );
    Ok(Report {
        status: if error.is_none() && cleanup_verified {
            "passed"
        } else {
            "failed"
        },
        prefix,
        bytes: bytes.len(),
        cleanup_verified,
        error_code: error.or((!cleanup_verified).then_some("object_probe_cleanup_unverified")),
    })
}
