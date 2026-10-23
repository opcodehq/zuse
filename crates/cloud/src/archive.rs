use serde_json::Value;
use zuse_protocol::cloud::{ArchivePhase, CloudArchiveStatus, RecoveryArchive, archive_deadline};

/// Reading archive progress never schedules work or asks a provider to wake.
pub fn status(row: &Value) -> CloudArchiveStatus {
    let config = &row["request_config"];
    let workspace = row["workspace_id"].as_str().unwrap_or_default();
    let archive = config
        .get("recoveryArchive")
        .map(|value| serde_json::from_value::<RecoveryArchive>(value.clone()));
    let parsed = archive.as_ref().and_then(|result| result.as_ref().ok());
    let valid = parsed.filter(|a| {
        a.valid_for(
            workspace,
            row["chat_id"].as_str().unwrap_or_default(),
            row["initial_session_id"].as_str().unwrap_or_default(),
        )
    });
    let invalid = archive.is_some() && valid.is_none();
    let state = row["state"].as_str().unwrap_or_default();
    CloudArchiveStatus {
        workspace_id: workspace.to_owned(),
        archived: state == "archived" || state == "archiving" || row["desired_state"] == "archived",
        machine_retained: row["provider_sandbox_id"].as_str().is_some(),
        backup_verified: valid.is_some_and(RecoveryArchive::backup_verified),
        compute_retired: valid.is_some_and(|a| {
            matches!(
                a.phase,
                ArchivePhase::Retired | ArchivePhase::Restoring | ArchivePhase::Restored
            )
        }),
        retention_deadline: archive_deadline(
            row["archive_requested_at"].as_i64(),
            row["archive_delete_at"].as_i64(),
        ),
        operation_id: valid.map(|a| a.id.clone()).or_else(|| {
            config["rustLifecycleOperation"]["id"]
                .as_str()
                .map(str::to_owned)
        }),
        lifecycle_phase: config["rustLifecycleOperation"]["phase"]
            .as_str()
            .map(str::to_owned),
        phase: valid.map(|a| a.phase.clone()),
        verified_chunks: valid.map_or(0, |a| a.verified_chunks),
        total_chunks: valid
            .and_then(|a| a.manifest.as_ref())
            .map_or(0, |m| m.chunks.len()),
        error_code: if invalid {
            Some("archive-metadata-invalid".into())
        } else {
            row["status_code"]
                .as_str()
                .filter(|s| {
                    s.contains("failed")
                        || s.contains("incomplete")
                        || *s == "archive-restore-image-required"
                        || s.starts_with("cloud_restore_")
                        || s.starts_with("cloud_archive_restore_")
                        || matches!(
                            *s,
                            "archive-source-missing"
                                | "archive-source-shared"
                                | "cloud_archive_operation_fenced"
                        )
                })
                .map(str::to_owned)
        },
    }
}
