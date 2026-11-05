//! Recognized launcher metadata is not a legacy engine database. Never discard it.
use super::*;
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

fn invalid() -> anyhow::Error {
    anyhow::anyhow!("runtime_storage_boot_artifact_invalid")
}
fn hex(value: &Value, length: usize) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.len() == length && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn keys(value: &Value, expected: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == expected.len() && expected.iter().all(|k| o.contains_key(*k)))
}
#[cfg(unix)]
fn metadata(path: &Path, uid: u32, private: bool) -> Result<std::fs::Metadata> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        !m.file_type().is_symlink()
            && m.uid() == uid
            && m.mode() & if private { 0o077 } else { 0o022 } == 0,
        "runtime_storage_boot_artifact_invalid"
    );
    Ok(m)
}
#[cfg(unix)]
fn json(path: &Path, uid: u32, private: bool) -> Result<Value> {
    use std::io::Read;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = file.metadata()?;
    ensure!(
        m.is_file()
            && m.uid() == uid
            && m.mode() & if private { 0o077 } else { 0o022 } == 0
            && m.len() <= 65536,
        "runtime_storage_boot_artifact_invalid"
    );
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 65536,
        "runtime_storage_boot_artifact_invalid"
    );
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}
#[cfg(unix)]
fn control(
    directory: &Path,
    bootstrap: &CloudRuntimeBootstrap,
    incarnation: &str,
    uid: u32,
) -> Result<()> {
    let record = json(&directory.join("cloud-runtime-supervision.json"), uid, true)?;
    ensure!(
        keys(&record, &["version", "identity", "receipts"]) && record["version"] == 1,
        "runtime_storage_boot_artifact_invalid"
    );
    let identity = &record["identity"];
    ensure!(
        keys(
            identity,
            &[
                "version",
                "workspaceId",
                "generation",
                "storageIncarnationId",
                "apiUrl"
            ]
        ) && identity["version"] == 1
            && identity["workspaceId"] == bootstrap.workspace_id
            && identity["generation"] == bootstrap.runtime_generation
            && identity["storageIncarnationId"] == incarnation,
        "runtime_storage_boot_artifact_invalid"
    );
    let api = identity["apiUrl"].as_str().ok_or_else(invalid)?;
    let mut gateway = reqwest::Url::parse(&bootstrap.gateway_url)?;
    let scheme = if gateway.scheme() == "wss" {
        "https"
    } else if gateway.scheme() == "ws" {
        "http"
    } else {
        return Err(invalid());
    };
    gateway.set_scheme(scheme).map_err(|_| invalid())?;
    ensure!(
        api.trim_end_matches('/') == gateway.origin().ascii_serialization(),
        "runtime_storage_boot_artifact_invalid"
    );
    let receipts = record["receipts"].as_object().ok_or_else(invalid)?;
    ensure!(
        !receipts.is_empty()
            && receipts.len() <= 32
            && receipts.iter().all(|(k, v)| !k.is_empty()
                && k.len() <= 160
                && k.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
                && hex(v, 64)),
        "runtime_storage_boot_artifact_invalid"
    );
    if let Some(operation) = bootstrap
        .extra
        .get("rustAllocationOperationId")
        .and_then(Value::as_str)
    {
        ensure!(
            receipts.contains_key(operation),
            "runtime_storage_boot_artifact_invalid"
        );
    }
    let ipc = directory.join(".runtime-supervision");
    ensure!(
        metadata(&ipc, uid, true)?.is_dir(),
        "runtime_storage_boot_artifact_invalid"
    );
    let mut count = 0;
    for entry in std::fs::read_dir(&ipc)? {
        let entry = entry?;
        let name = entry.file_name();
        let m = metadata(&entry.path(), uid, true)?;
        use std::os::unix::fs::FileTypeExt;
        ensure!(
            if name == "control.sock" {
                m.file_type().is_socket()
            } else {
                (name == "supervisor.lock" || name == "supervisor.json") && m.is_file()
            },
            "runtime_storage_boot_artifact_invalid"
        );
        count += 1;
    }
    ensure!(count == 3, "runtime_storage_boot_artifact_invalid");
    let resident = json(&ipc.join("supervisor.json"), uid, true)?;
    ensure!(
        keys(&resident, &["version", "bootNonce", "residentPid"])
            && resident["version"] == 1
            && hex(&resident["bootNonce"], 32)
            && resident["residentPid"]
                .as_u64()
                .is_some_and(|p| p > 0 && p <= i32::MAX as u64),
        "runtime_storage_boot_artifact_invalid"
    );
    Ok(())
}
/// Only this exact metadata shape may accompany a new enrolled native store.
#[cfg(unix)]
pub(super) fn recognized(
    directory: &Path,
    name: &str,
    bootstrap: &CloudRuntimeBootstrap,
    incarnation: &str,
) -> Result<bool> {
    let uid = std::fs::symlink_metadata(directory)?.uid();
    ensure!(
        uid == unsafe { libc::geteuid() },
        "runtime_storage_boot_artifact_invalid"
    );
    match name {
        ".runtime-supervision" | "cloud-runtime-supervision.json" => {
            control(directory, bootstrap, incarnation, uid)?;
            Ok(true)
        }
        "cloud-harness-release.json" => {
            let value = json(&directory.join(name), uid, false)?;
            ensure!(
                keys(&value, &["release"]) && hex(&value["release"], 64),
                "runtime_storage_boot_artifact_invalid"
            );
            if let Ok(expected) = std::env::var("ZUSE_HARNESS_RELEASE") {
                ensure!(
                    value["release"] == expected,
                    "runtime_storage_boot_artifact_invalid"
                );
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
#[cfg(not(unix))]
pub(super) fn recognized(_: &Path, _: &str, _: &CloudRuntimeBootstrap, _: &str) -> Result<bool> {
    Ok(false)
}
