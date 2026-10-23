//! Durable original-format guest identity; retries keep keys and renewal request IDs.
use anyhow::{Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
use rand::rngs::OsRng;
use rsa::{
    BigUint, Oaep, RsaPrivateKey,
    traits::{PrivateKeyParts, PublicKeyParts},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;
use zuse_protocol::cloud::{CloudRuntimeBootstrap, CloudRuntimeBootstrapRequest};
pub const FILE: &str = "cloud-runtime-identity.json";
#[derive(Clone)]
pub struct IdentityBinding {
    pub workspace: String,
    pub api_url: String,
    pub incarnation: String,
    pub generation: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    version: u8,
    workspace_id: String,
    api_url: String,
    storage_incarnation_id: String,
    account_id: Option<String>,
    boot_token_hash: String,
    generation: Option<i64>,
    credential_private_jwk: String,
    signing_private_jwk: String,
    credential_public_jwk: String,
    signing_public_jwk: String,
    bootstrap: Option<CloudRuntimeBootstrap>,
    bootstrap_acknowledged: bool,
    pending_renewal_id: Option<String>,
}
pub struct Identity {
    path: PathBuf,
    stored: Stored,
    credential: RsaPrivateKey,
    signing: SigningKey,
}
impl Identity {
    /// The caller holds the lifetime data lock before reading or creating identity.
    pub fn open(directory: &Path, binding: &IdentityBinding, boot: Option<&str>) -> Result<Self> {
        ensure!(
            !binding.workspace.is_empty()
                && binding.workspace.len() <= 128
                && !binding.incarnation.is_empty()
                && binding.incarnation.len() <= 128
                && binding.generation > 0,
            "runtime_identity_binding_invalid"
        );
        ensure!(
            boot.is_none_or(|s| !s.is_empty() && s.len() <= 512),
            "runtime_boot_token_invalid"
        );
        let path = directory.join(FILE);
        ensure!(
            std::fs::metadata(&path)
                .map(|m| m.len() <= 2 * 1024 * 1024)
                .unwrap_or(true),
            "runtime_identity_unreadable"
        );
        let prior = match std::fs::read(&path) {
            Ok(bytes) => Some(
                serde_json::from_slice::<Stored>(&Zeroizing::new(bytes))
                    .map_err(|_| anyhow::anyhow!("runtime_identity_unreadable"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => bail!("runtime_identity_unreadable"),
        };
        let hash = boot.map(digest);
        let mut stored = match prior {
            Some(mut stored) => {
                ensure!(
                    stored.version == 1
                        && stored.workspace_id == binding.workspace
                        && stored.api_url == binding.api_url
                        && stored.storage_incarnation_id == binding.incarnation,
                    "runtime_identity_binding_mismatch"
                );
                if hash.as_ref().is_some_and(|h| h != &stored.boot_token_hash) {
                    stored.boot_token_hash = hash.clone().unwrap_or_default();
                    stored.bootstrap = None;
                    stored.bootstrap_acknowledged = false;
                    stored.pending_renewal_id = None;
                    stored.generation = None;
                }
                ensure!(
                    stored.generation.is_none_or(|g| g == binding.generation),
                    "runtime_identity_generation_mismatch"
                );
                stored
            }
            None => {
                ensure!(
                    boot.is_some_and(|s| !s.is_empty() && s.len() <= 512),
                    "runtime_identity_missing"
                );
                let key = RsaPrivateKey::new(&mut OsRng, 2048)
                    .map_err(|_| anyhow::anyhow!("runtime_identity_key_generation_failed"))?;
                let seed: [u8; 32] = rand::random();
                let signing = SigningKey::from_bytes(&seed);
                let enc = |n: &BigUint| B64.encode(n.to_bytes_be());
                let public =
                    json!({"kty":"RSA","n":enc(key.n()),"e":enc(key.e()),"alg":"RSA-OAEP-256"});
                let mut private = public.clone();
                private["d"] = json!(enc(key.d()));
                private["p"] = json!(enc(&key.primes()[0]));
                private["q"] = json!(enc(&key.primes()[1]));
                private["dp"] = json!(enc(key
                    .dp()
                    .ok_or_else(|| anyhow::anyhow!("runtime_identity_invalid_key"))?));
                private["dq"] = json!(enc(key
                    .dq()
                    .ok_or_else(|| anyhow::anyhow!("runtime_identity_invalid_key"))?));
                private["qi"] = json!(enc(&key
                    .crt_coefficient()
                    .ok_or_else(|| anyhow::anyhow!("runtime_identity_invalid_key"))?));
                let ed_public = json!({"kty":"OKP","crv":"Ed25519","x":B64.encode(signing.verifying_key().as_bytes())});
                let mut ed_private = ed_public.clone();
                ed_private["d"] = json!(B64.encode(seed));
                Stored {
                    version: 1,
                    workspace_id: binding.workspace.clone(),
                    api_url: binding.api_url.clone(),
                    storage_incarnation_id: binding.incarnation.clone(),
                    account_id: None,
                    boot_token_hash: hash.unwrap_or_default(),
                    generation: None,
                    credential_private_jwk: private.to_string(),
                    credential_public_jwk: public.to_string(),
                    signing_private_jwk: ed_private.to_string(),
                    signing_public_jwk: ed_public.to_string(),
                    bootstrap: None,
                    bootstrap_acknowledged: false,
                    pending_renewal_id: None,
                }
            }
        };
        let private: Value = serde_json::from_str(&stored.credential_private_jwk)
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        let number = |field: &str| -> Result<BigUint> {
            Ok(BigUint::from_bytes_be(
                &B64.decode(private[field].as_str().unwrap_or_default())
                    .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?,
            ))
        };
        let credential = RsaPrivateKey::from_components(
            number("n")?,
            number("e")?,
            number("d")?,
            vec![number("p")?, number("q")?],
        )
        .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        credential
            .validate()
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        ensure!(
            credential.n().bits() >= 2048 && credential.n().bits() <= 8192,
            "runtime_identity_invalid_key"
        );
        let public: Value = serde_json::from_str(&stored.credential_public_jwk)
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        ensure!(
            public["n"] == private["n"]
                && public["e"] == private["e"]
                && public["kty"] == "RSA"
                && private["kty"] == "RSA",
            "runtime_identity_invalid_key"
        );
        let ed: Value = serde_json::from_str(&stored.signing_private_jwk)
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        let seed = Zeroizing::new(
            B64.decode(ed["d"].as_str().unwrap_or_default())
                .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?,
        );
        let signing = SigningKey::from_bytes(
            seed.as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?,
        );
        let public: Value = serde_json::from_str(&stored.signing_public_jwk)
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        ensure!(
            ed["kty"] == "OKP"
                && ed["crv"] == "Ed25519"
                && ed["x"] == B64.encode(signing.verifying_key().as_bytes())
                && public["x"] == ed["x"]
                && public["kty"] == ed["kty"]
                && public["crv"] == ed["crv"],
            "runtime_identity_invalid_key"
        );
        if let Some(bootstrap) = &stored.bootstrap {
            validate_binding(&stored, bootstrap, binding.generation)?;
        }
        // Keep an old account binding across a fresh boot authorization.
        stored.version = 1;
        let identity = Self {
            path,
            stored,
            credential,
            signing,
        };
        identity.save()?;
        Ok(identity)
    }
    fn save(&self) -> Result<()> {
        atomic_write(
            &self.path,
            &Zeroizing::new(serde_json::to_vec(&self.stored)?),
        )
    }
    pub fn bootstrap(&self) -> Option<&CloudRuntimeBootstrap> {
        self.stored.bootstrap.as_ref()
    }
    pub fn has_pending_renewal(&self) -> bool {
        self.stored.pending_renewal_id.is_some()
    }
    pub fn acknowledged(&self) -> bool {
        self.stored.bootstrap_acknowledged
    }
    pub fn grant_recipient(
        &self,
        bootstrap: &CloudRuntimeBootstrap,
    ) -> Result<crate::grants::GrantRecipient> {
        crate::grants::GrantRecipient::new(
            self,
            bootstrap,
            self.credential.clone(),
            self.stored.credential_public_jwk.clone(),
        )
    }
    pub fn request(&self) -> CloudRuntimeBootstrapRequest {
        CloudRuntimeBootstrapRequest {
            credential_public_jwk: self.stored.credential_public_jwk.clone(),
            signing_public_jwk: self.stored.signing_public_jwk.clone(),
            capabilities: Some(vec![
                "rust-rpc-v1".into(),
                "rust-session-commands-v1".into(),
            ]),
            rpc_protocol: Some(zuse_protocol::cloud_gateway::RUST_RPC.into()),
        }
    }
    pub fn commit_bootstrap(
        &mut self,
        bootstrap: CloudRuntimeBootstrap,
        generation: i64,
    ) -> Result<()> {
        validate_binding(&self.stored, &bootstrap, generation)?;
        self.open_transcript_key(&bootstrap.sealed_transcript_key)?;
        self.stored.account_id = Some(bootstrap.zuse_account_id.clone());
        self.stored.generation = Some(bootstrap.runtime_generation);
        self.stored.bootstrap = Some(bootstrap);
        self.stored.pending_renewal_id = None;
        self.save()
    }
    pub fn acknowledge(&mut self) -> Result<()> {
        self.stored.bootstrap_acknowledged = true;
        self.save()
    }
    /// The engine queue owns the launch after the authority accepts its readiness receipt.
    pub fn complete_launch(&mut self, command: &str) -> Result<()> {
        let bootstrap = self
            .stored
            .bootstrap
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        if let Some(intent) = &bootstrap.launch_intent {
            ensure!(
                intent.command_id == command,
                "runtime_launch_identity_mismatch"
            );
            bootstrap.launch_intent = None;
            self.save()?;
        }
        Ok(())
    }
    pub fn begin_renewal(&mut self) -> Result<String> {
        if self.stored.pending_renewal_id.is_none() {
            self.stored.pending_renewal_id = Some(uuid::Uuid::new_v4().to_string());
            self.save()?;
        }
        Ok(self.stored.pending_renewal_id.clone().unwrap_or_default())
    }
    pub fn renewal_proof(&self, request: &str, now: i64) -> Result<String> {
        let boot = self
            .bootstrap()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        let der = self
            .signing
            .to_pkcs8_der()
            .map_err(|_| anyhow::anyhow!("runtime_identity_invalid_key"))?;
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::EdDSA);
        header.typ = Some("workspace-runtime-renewal+jwt".into());
        jsonwebtoken::encode(&header,&json!({"workspaceId":self.stored.workspace_id,"requestId":request,"generation":boot.runtime_generation,"gatewayEpoch":boot.gateway_epoch,"aud":self.stored.api_url,"iat":now/1000,"exp":now/1000+60}),&jsonwebtoken::EncodingKey::from_ed_der(der.as_bytes())).map_err(|_|anyhow::anyhow!("runtime_proof_failed"))
    }
    pub fn commit_renewal(&mut self, response: &Value) -> Result<()> {
        let boot = self
            .stored
            .bootstrap
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        ensure!(
            response["workspaceId"] == boot.workspace_id
                && response["requestId"].as_str() == self.stored.pending_renewal_id.as_deref()
                && response["generation"] == boot.runtime_generation
                && response["gatewayEpoch"] == boot.gateway_epoch,
            "runtime_renewal_binding_mismatch"
        );
        let credential = response["runtimeCredential"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(|| anyhow::anyhow!("runtime_renewal_binding_mismatch"))?;
        let expiry = response["expiresAt"]
            .as_i64()
            .filter(|e| *e > crate::now_ms())
            .ok_or_else(|| anyhow::anyhow!("runtime_renewal_binding_mismatch"))?;
        boot.runtime_credential = credential.into();
        boot.runtime_credential_expires_at = expiry;
        self.stored.pending_renewal_id = None;
        self.save()
    }
    pub fn open_transcript_key(&self, sealed: &str) -> Result<Zeroizing<String>> {
        use aes_gcm::{
            Aes256Gcm, KeyInit, Nonce,
            aead::{Aead, Payload},
        };
        ensure!(sealed.len() <= 32768, "runtime_transcript_key_invalid");
        let parts: Vec<_> = sealed.split('.').collect();
        ensure!(parts.len() == 5, "runtime_transcript_key_invalid");
        let decode = |part: &str| {
            B64.decode(part)
                .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))
        };
        let header: Value = serde_json::from_slice(&decode(parts[0])?)
            .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))?;
        ensure!(
            header["alg"] == "RSA-OAEP-256"
                && header["enc"] == "A256GCM"
                && header.get("crit").is_none(),
            "runtime_transcript_key_invalid"
        );
        let cek = Zeroizing::new(
            self.credential
                .decrypt(Oaep::new::<Sha256>(), &decode(parts[1])?)
                .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))?,
        );
        let iv = decode(parts[2])?;
        let mut ciphertext = decode(parts[3])?;
        let tag = decode(parts[4])?;
        ensure!(
            iv.len() == 12 && tag.len() == 16,
            "runtime_transcript_key_invalid"
        );
        ciphertext.extend_from_slice(&tag);
        let plain = Zeroizing::new(
            Aes256Gcm::new_from_slice(&cek)
                .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))?
                .decrypt(
                    Nonce::from_slice(&iv),
                    Payload {
                        msg: &ciphertext,
                        aad: parts[0].as_bytes(),
                    },
                )
                .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))?,
        );
        let text = std::str::from_utf8(&plain)
            .map_err(|_| anyhow::anyhow!("runtime_transcript_key_invalid"))?;
        ensure!(decode(text)?.len() == 32, "runtime_transcript_key_invalid");
        Ok(Zeroizing::new(text.into()))
    }
}
fn validate_binding(stored: &Stored, boot: &CloudRuntimeBootstrap, generation: i64) -> Result<()> {
    ensure!(
        boot.workspace_id == stored.workspace_id
            && !boot.zuse_account_id.is_empty()
            && stored
                .account_id
                .as_ref()
                .is_none_or(|a| a == &boot.zuse_account_id)
            && generation == boot.runtime_generation
            && boot.gateway_epoch > 0
            && !boot.runtime_credential.is_empty(),
        "runtime_identity_binding_mismatch"
    );
    Ok(())
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("runtime_identity_write_failed"))?;
    let temp = parent.join(format!(".runtime-write-{}.next", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}
fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
