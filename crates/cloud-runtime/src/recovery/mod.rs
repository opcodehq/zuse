//! Fenced full-workspace recovery. Uses the original manifest, tar roots and chunk cipher.
mod files;
#[cfg(test)]
mod tests;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;
use zuse_protocol::cloud::ArchiveManifest;
const CHUNK: usize = 4 * 1024 * 1024;
const MAX_WIRE: usize = 6 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub operation_id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub session_id: String,
    #[serde(default)]
    pub rust_storage: Option<RustStorage>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RustStorage {
    pub database: PathBuf,
    pub account_id: String,
    pub storage_incarnation_id: String,
}
impl Binding {
    fn validate(&self) -> Result<()> {
        for value in [
            &self.operation_id,
            &self.workspace_id,
            &self.chat_id,
            &self.session_id,
        ] {
            ensure!(
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "recovery_identity_invalid"
            );
        }
        if let Some(storage) = &self.rust_storage {
            ensure!(
                files::relative(&storage.database)
                    && !storage.account_id.is_empty()
                    && !storage.storage_incarnation_id.is_empty(),
                "recovery_storage_invalid"
            );
        }
        Ok(())
    }
    fn check(&self, m: &ArchiveManifest) -> Result<()> {
        ensure!(
            m.valid()
                && m.operation_id == self.operation_id
                && m.workspace_id == self.workspace_id
                && m.chat_id == self.chat_id
                && m.session_id == self.session_id,
            "recovery_archive_identity_mismatch"
        );
        Ok(())
    }
    fn aad(&self, index: usize) -> Vec<u8> {
        format!(
            "zuse-workspace-archive-v1\n{}\n{}\n{index}",
            self.workspace_id, self.operation_id
        )
        .into_bytes()
    }
}
pub struct Key(Zeroizing<Vec<u8>>);
impl Key {
    pub fn parse(encoded: &str) -> Result<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
        ensure!(bytes.len() == 32, "recovery_key_invalid");
        Ok(Self(Zeroizing::new(bytes)))
    }
    fn seal(&self, binding: &Binding, index: usize, plain: &[u8]) -> Result<Vec<u8>> {
        ensure!(plain.len() <= CHUNK, "recovery_chunk_invalid");
        let mut nonce = [0; 12];
        OsRng.fill_bytes(&mut nonce);
        let mut bytes = nonce.to_vec();
        bytes.extend(
            Aes256Gcm::new_from_slice(&self.0)
                .map_err(|_| anyhow::anyhow!("recovery_key_invalid"))?
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: plain,
                        aad: &binding.aad(index),
                    },
                )
                .map_err(|_| anyhow::anyhow!("recovery_encrypt_failed"))?,
        );
        Ok(STANDARD.encode(bytes).into_bytes())
    }
    fn open(&self, binding: &Binding, index: usize, wire: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(wire.len() <= MAX_WIRE, "recovery_chunk_invalid");
        let bytes = STANDARD.decode(wire)?;
        ensure!(
            (28..=CHUNK + 28).contains(&bytes.len()),
            "recovery_chunk_invalid"
        );
        let plain = Aes256Gcm::new_from_slice(&self.0)
            .map_err(|_| anyhow::anyhow!("recovery_key_invalid"))?
            .decrypt(
                Nonce::from_slice(&bytes[..12]),
                Payload {
                    msg: &bytes[12..],
                    aad: &binding.aad(index),
                },
            )
            .map_err(|_| anyhow::anyhow!("recovery_chunk_authentication_failed"))?;
        Ok(Zeroizing::new(plain))
    }
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_digest(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    let mut file = File::open(path)?;
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::identity::atomic_write(path, bytes)
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
#[async_trait]
pub trait Objects: Send + Sync {
    async fn get(&self, name: &str) -> Result<Vec<u8>>;
    async fn put(&self, name: &str, bytes: &[u8]) -> Result<()>;
}
pub struct Remote {
    http: reqwest::Client,
    url: reqwest::Url,
    token: Zeroizing<String>,
}
impl Remote {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        let mut url = reqwest::Url::parse(url)?;
        let local = url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            (url.scheme() == "https" || url.scheme() == "http" && local)
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && !token.is_empty(),
            "recovery_transport_invalid"
        );
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("recovery_transport_invalid"))?
            .pop_if_empty()
            .push("");
        Ok(Self {
            url,
            token: Zeroizing::new(token.into()),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()?,
        })
    }
    async fn request(&self, name: &str, body: Option<&[u8]>) -> Result<Vec<u8>> {
        ensure!(
            name == "manifest"
                || name.len() == 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "recovery_object_invalid"
        );
        ensure!(
            body.is_none_or(|b| b.len() <= MAX_WIRE),
            "recovery_object_too_large"
        );
        let url = self.url.join(name)?;
        let req = match body {
            Some(b) => self.http.put(url).body(b.to_vec()),
            None => self.http.get(url),
        };
        let mut response = req
            .bearer_auth(&*self.token)
            .header("content-type", "text/plain")
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("recovery_transport_failed"))?;
        ensure!(
            response.status().is_success(),
            "recovery_transport_rejected"
        );
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len()
                    <= if name == "manifest" {
                        1024 * 1024
                    } else {
                        MAX_WIRE
                    },
                "recovery_object_too_large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
#[async_trait]
impl Objects for Remote {
    async fn get(&self, name: &str) -> Result<Vec<u8>> {
        self.request(name, None).await
    }
    async fn put(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.request(name, Some(bytes)).await?;
        Ok(())
    }
}
#[derive(Clone)]
pub struct Layout {
    pub data: PathBuf,
    pub repositories: PathBuf,
    pub home: PathBuf,
    pub work: PathBuf,
}
pub struct Recovery {
    binding: Binding,
    layout: Layout,
    key: Key,
    _data_lock: crate::DataLock,
    _engine_lock: zuse_engine::InstanceLock,
}
impl Recovery {
    /// The authority must fence execution first. Both locks also reject a running guest.
    pub fn acquire(binding: Binding, layout: Layout, key: Key) -> Result<Self> {
        Self::acquire_inherited(binding, layout, key, None)
    }
    pub fn acquire_inherited(
        binding: Binding,
        layout: Layout,
        key: Key,
        inherited: Option<i32>,
    ) -> Result<Self> {
        binding.validate()?;
        files::validate_layout(&layout)?;
        let data_lock = crate::DataLock::acquire(&layout.data, inherited)?;
        let engine_lock = zuse_engine::InstanceLock::acquire(&layout.data)?;
        files::private_directory(&layout.work)?;
        let owner = layout.work.join("binding.json");
        let bytes = serde_json::to_vec(&binding)?;
        if owner.exists() {
            let prior: Binding = serde_json::from_slice(&std::fs::read(&owner)?)?;
            ensure!(prior == binding, "recovery_operation_mismatch");
        } else {
            ensure!(
                std::fs::read_dir(&layout.work)?.next().is_none(),
                "recovery_work_directory_unowned"
            );
            write(&owner, &bytes)?;
        }
        Ok(Self {
            binding,
            layout,
            key,
            _data_lock: data_lock,
            _engine_lock: engine_lock,
        })
    }
    fn manifest(&self) -> PathBuf {
        self.layout.work.join("manifest.json")
    }
    pub async fn backup(&self, objects: &dyn Objects) -> Result<ArchiveManifest> {
        let manifest = if self.manifest().exists() {
            let m: ArchiveManifest = serde_json::from_slice(&std::fs::read(self.manifest())?)?;
            self.binding.check(&m)?;
            m
        } else {
            let archive = self.layout.work.join("capture.tar.gz");
            files::capture(&self.layout, &archive)?;
            let verify = self.layout.work.join("capture-verification");
            files::extract(&archive, &verify)?;
            files::validate_stage(&verify, &self.binding)?;
            let mut source = File::open(&archive)?;
            let mut chunks = vec![];
            loop {
                let mut plain = Zeroizing::new(vec![0; CHUNK]);
                let mut n = 0;
                while n < CHUNK {
                    let got = source.read(&mut plain[n..])?;
                    if got == 0 {
                        break;
                    }
                    n += got;
                }
                if n == 0 {
                    break;
                }
                ensure!(chunks.len() < 8192, "recovery_archive_too_large");
                let wire = self.key.seal(&self.binding, chunks.len(), &plain[..n])?;
                let hash = digest(&wire);
                write(&self.layout.work.join(&hash), &wire)?;
                chunks.push(hash);
            }
            let m = ArchiveManifest {
                version: 1,
                operation_id: self.binding.operation_id.clone(),
                workspace_id: self.binding.workspace_id.clone(),
                chat_id: self.binding.chat_id.clone(),
                session_id: self.binding.session_id.clone(),
                chunks,
                archive_sha256: file_digest(&archive)?,
            };
            self.binding.check(&m)?;
            write(&self.manifest(), &serde_json::to_vec(&m)?)?;
            m
        };
        for hash in &manifest.chunks {
            let wire = std::fs::read(self.layout.work.join(hash))?;
            ensure!(digest(&wire) == *hash, "recovery_local_chunk_changed");
            objects.put(hash, &wire).await?;
        }
        // The original upload capability is write-only. Publish the manifest after all chunks.
        objects
            .put("manifest", &serde_json::to_vec(&manifest)?)
            .await?;
        write(
            &self.layout.work.join("uploaded"),
            manifest.archive_sha256.as_bytes(),
        )?;
        Ok(manifest)
    }
    /// Privileged control-plane verification, separate from the guest's write-only upload grant.
    /// The caller persists its retirement barrier only after this returns successfully.
    pub async fn verify(&self, objects: &dyn Objects) -> Result<ArchiveManifest> {
        let bytes = objects.get("manifest").await?;
        ensure!(bytes.len() <= 1024 * 1024, "recovery_manifest_too_large");
        let manifest: ArchiveManifest = serde_json::from_slice(&bytes)?;
        self.binding.check(&manifest)?;
        // Never substitute an earlier local verification for reading every remote chunk again.
        let downloaded = self
            .download(objects, &manifest, "remote-verification.tar.gz", false)
            .await?;
        let verify = self.layout.work.join("remote-verification");
        files::extract(&downloaded, &verify)?;
        files::validate_stage(&verify, &self.binding)?;
        write(
            &self.layout.work.join("verified"),
            manifest.archive_sha256.as_bytes(),
        )?;
        Ok(manifest)
    }
    async fn download(
        &self,
        objects: &dyn Objects,
        manifest: &ArchiveManifest,
        name: &str,
        reuse: bool,
    ) -> Result<PathBuf> {
        self.binding.check(manifest)?;
        let destination = self.layout.work.join(name);
        if reuse && destination.exists() && file_digest(&destination)? == manifest.archive_sha256 {
            return Ok(destination);
        }
        let next = self
            .layout
            .work
            .join(format!("download-{}.next", uuid::Uuid::new_v4()));
        let mut out = private_file(&next)?;
        let mut hash = Sha256::new();
        for (index, digest_expected) in manifest.chunks.iter().enumerate() {
            let wire = objects.get(digest_expected).await?;
            ensure!(
                digest(&wire) == *digest_expected,
                "recovery_chunk_checksum_mismatch"
            );
            let plain = self.key.open(&self.binding, index, &wire)?;
            out.write_all(&plain)?;
            hash.update(&plain);
        }
        out.sync_all()?;
        ensure!(
            format!("{:x}", hash.finalize()) == manifest.archive_sha256,
            "recovery_archive_checksum_mismatch"
        );
        std::fs::rename(next, &destination)?;
        File::open(&self.layout.work)?.sync_all()?;
        Ok(destination)
    }
    pub async fn restore(&self, objects: &dyn Objects) -> Result<ArchiveManifest> {
        let bytes = objects.get("manifest").await?;
        ensure!(bytes.len() <= 1024 * 1024, "recovery_manifest_too_large");
        let manifest: ArchiveManifest = serde_json::from_slice(&bytes)?;
        self.binding.check(&manifest)?;
        if self.manifest().exists() {
            let local: serde_json::Value =
                serde_json::from_slice(&std::fs::read(self.manifest())?)?;
            ensure!(
                local == serde_json::to_value(&manifest)?,
                "recovery_manifest_changed"
            );
        } else {
            write(&self.manifest(), &bytes)?;
        }
        let receipt = self.layout.work.join("restored");
        if receipt.exists() {
            ensure!(
                std::fs::read_to_string(&receipt)? == manifest.archive_sha256,
                "recovery_receipt_invalid"
            );
            files::validate_installed(&self.layout, &self.binding)?;
            return Ok(manifest);
        }
        let archive = self
            .download(objects, &manifest, "restore.tar.gz", true)
            .await?;
        let stage = self.layout.work.join("restore");
        let extracted = self.layout.work.join("extracted");
        if !extracted.exists() {
            files::extract(&archive, &stage)?;
            files::validate_stage(&stage, &self.binding)?;
            write(&extracted, manifest.archive_sha256.as_bytes())?;
        }
        ensure!(
            std::fs::read_to_string(&extracted)? == manifest.archive_sha256,
            "recovery_stage_invalid"
        );
        files::install(&self.layout, &self.binding, &stage)?;
        files::validate_installed(&self.layout, &self.binding)?;
        write(&receipt, manifest.archive_sha256.as_bytes())?;
        Ok(manifest)
    }
}

struct Secret(Zeroizing<String>);
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(d)?)))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(flatten)]
    binding: Binding,
    home: PathBuf,
    #[serde(default = "repositories_default")]
    repositories_path: PathBuf,
    url: String,
    token: Secret,
    key: Secret,
    mode: String,
    expires_at_ms: i64,
}
fn repositories_default() -> PathBuf {
    PathBuf::from("/home/repos")
}
/// Original helper environment contract. No runtime or account credentials are accepted or created.
pub enum Outcome {
    Uploaded(ArchiveManifest),
    Restored(ArchiveManifest),
}
impl Outcome {
    pub fn phase(&self) -> &'static str {
        match self {
            Self::Uploaded(_) => "workspace_archive_uploaded",
            Self::Restored(_) => "workspace_archive_restored",
        }
    }
}
pub async fn run(data: PathBuf, raw: &str) -> Result<Outcome> {
    let config: Config = serde_json::from_str(raw)?;
    ensure!(
        config.expires_at_ms > crate::now_ms(),
        "recovery_capability_expired"
    );
    config.binding.validate()?;
    let layout = Layout {
        data,
        repositories: config.repositories_path,
        work: config
            .home
            .join(".zuse-recovery")
            .join(&config.binding.operation_id),
        home: config.home,
    };
    let inherited = std::env::var("ZUSE_WORKSPACE_RUNTIME_LOCK_FD")
        .ok()
        .map(|fd| fd.parse::<i32>())
        .transpose()?;
    let helper = Recovery::acquire_inherited(
        config.binding,
        layout,
        Key::parse(&config.key.0)?,
        inherited,
    )?;
    let remote = Remote::new(&config.url, &config.token.0)?;
    match config.mode.as_str() {
        "backup" => helper.backup(&remote).await.map(Outcome::Uploaded),
        "restore" => helper.restore(&remote).await.map(Outcome::Restored),
        _ => anyhow::bail!("recovery_mode_invalid"),
    }
}
