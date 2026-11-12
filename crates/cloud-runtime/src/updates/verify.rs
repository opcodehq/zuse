//! Release trust: Ed25519 signature, architecture and storage compatibility
//! before any download; SHA-256 of the uncompressed binary after it.
use super::{ARTIFACT_INVALID, STORAGE_INCOMPATIBLE, valid_sha};
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use std::path::Path;
use zuse_protocol::cloud_runtime_updates::SignedRelease;

pub fn release_key(encoded: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = STANDARD
        .decode(encoded)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!("runtime_update_key_invalid"))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| anyhow!("runtime_update_key_invalid"))
}

/// Errors carry the attestation code as their message.
pub fn release(
    release: &SignedRelease,
    signature: &str,
    key: &VerifyingKey,
    architecture: &str,
) -> Result<()> {
    let signature: [u8; 64] = STANDARD
        .decode(signature)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| anyhow!(ARTIFACT_INVALID))?;
    key.verify_strict(&release.signing_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| anyhow!(ARTIFACT_INVALID))?;
    ensure!(
        valid_sha(&release.sha256)
            && release.architecture == architecture
            && !release.version.is_empty()
            && release.version.len() <= 128,
        ARTIFACT_INVALID
    );
    // Never touch storage formats: the release must read what this computer has.
    ensure!(
        release
            .storage_formats
            .iter()
            .any(|f| f == zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT),
        STORAGE_INCOMPATIBLE
    );
    Ok(())
}

pub fn binary(path: &Path, release: &SignedRelease) -> Result<()> {
    let digest = super::sha256_file(path).map_err(|_| anyhow!(ARTIFACT_INVALID))?;
    ensure!(digest == release.sha256, ARTIFACT_INVALID);
    Ok(())
}
