//! Profile-bound asymmetric identity. Only its public JWK leaves the host.
use super::{Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

pub struct HostIdentity {
    key: SigningKey,
    device: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    device: String,
    seed: String,
}
impl Drop for Saved {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.seed.zeroize();
    }
}
impl HostIdentity {
    pub fn open(root: &Path, device: &str) -> Result<Self> {
        if !super::valid_id(device) {
            return Err(Error::Identity);
        }
        let directory = root.join("managed-access");
        std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::Storage)?;
        }
        let path = directory.join("identity.json");
        if !path.try_exists().map_err(|_| Error::Storage)? {
            let mut seed = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(&mut *seed);
            let saved = Saved {
                version: 1,
                device: device.into(),
                seed: URL_SAFE_NO_PAD.encode(*seed),
            };
            let mut temporary =
                tempfile::NamedTempFile::new_in(&directory).map_err(|_| Error::Storage)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temporary
                    .as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|_| Error::Storage)?;
            }
            let bytes = Zeroizing::new(serde_json::to_vec(&saved).map_err(|_| Error::Storage)?);
            temporary
                .write_all(&bytes)
                .and_then(|_| temporary.as_file().sync_all())
                .map_err(|_| Error::Storage)?;
            match temporary.persist_noclobber(&path) {
                Ok(_) => {}
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Error::Storage),
            }
            #[cfg(unix)]
            std::fs::File::open(&directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Storage)?;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| Error::Storage)?;
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(Error::Identity);
        }
        let file = std::fs::File::open(path).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Storage)?;
        }
        let mut bytes = Zeroizing::new(vec![]);
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > 4096 {
            return Err(Error::Identity);
        }
        let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Error::Identity)?;
        if saved.version != 1 || saved.device != device {
            return Err(Error::Identity);
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        if URL_SAFE_NO_PAD
            .decode_slice(&saved.seed, &mut *seed)
            .map_err(|_| Error::Identity)?
            != 32
        {
            return Err(Error::Identity);
        }
        Ok(Self {
            key: SigningKey::from_bytes(&seed),
            device: device.into(),
        })
    }
    pub fn device(&self) -> &str {
        &self.device
    }
    pub fn public_jwk(&self) -> String {
        json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes())}).to_string()
    }
    pub fn link_proof(&self, challenge: &str, issuer: &str) -> Result<String> {
        if challenge.is_empty() || challenge.len() > 4096 {
            return Err(Error::Invalid);
        }
        zuse_remote::normalized_origin(issuer).map_err(|_| Error::Invalid)?;
        let now = chrono::Utc::now().timestamp();
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("environment-link-proof+jwt".into());
        let der = self.key.to_pkcs8_der().map_err(|_| Error::Identity)?;
        jsonwebtoken::encode(&header,&json!({"challenge":challenge,"environmentId":self.device,"aud":issuer,"iat":now,"exp":now+300}),&EncodingKey::from_ed_der(der.as_bytes())).map_err(|_|Error::Identity)
    }
}
