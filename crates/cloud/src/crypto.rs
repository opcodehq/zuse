//! Compatibility with existing AES-GCM envelopes, API digests and runtime JWE grants.
//! Encryption uses RustCrypto primitives; the envelope/AAD are existing cloud contracts.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use hmac::{Hmac, Mac};
use rsa::{BigUint, Oaep, RsaPublicKey, traits::PublicKeyParts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAX_ENVELOPE: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoError {
    InvalidKey,
    InvalidEnvelope,
    AuthenticationFailed,
    EncryptionFailed,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    #[serde(rename = "keyId", default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    iv: String,
    ciphertext: String,
}

/// Never Debug/Serialize: this key preserves existing account-scoped ciphertext.
pub struct MasterKey(Zeroizing<[u8; 32]>);

fn decode(value: &str) -> Result<Vec<u8>, CryptoError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .or_else(|_| STANDARD.decode(value))
        .or_else(|_| STANDARD_NO_PAD.decode(value))
        .map_err(|_| CryptoError::InvalidEnvelope)
}
fn encrypt(key: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Envelope, CryptoError> {
    if plaintext.len() > 6 * 1024 * 1024 {
        return Err(CryptoError::InvalidEnvelope);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKey)?;
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::EncryptionFailed)?;
    Ok(Envelope {
        version: 1,
        key_id: None,
        iv: URL_SAFE_NO_PAD.encode(iv),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    })
}
fn decrypt(key: &[u8], aad: &[u8], envelope: Envelope) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if envelope.version != 1 || envelope.ciphertext.len() > MAX_ENVELOPE || envelope.iv.len() > 32 {
        return Err(CryptoError::InvalidEnvelope);
    }
    let iv = decode(&envelope.iv)?;
    if iv.len() != 12 {
        return Err(CryptoError::InvalidEnvelope);
    }
    let ciphertext = decode(&envelope.ciphertext)?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKey)?;
    cipher
        .decrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::AuthenticationFailed)
}

impl MasterKey {
    pub fn parse(encoded: &str) -> Result<Self, CryptoError> {
        if encoded.len() > 128 {
            return Err(CryptoError::InvalidKey);
        }
        let raw = Zeroizing::new(decode(encoded).map_err(|_| CryptoError::InvalidKey)?);
        let key: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self(Zeroizing::new(key)))
    }
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Result<String, CryptoError> {
        serde_json::to_string(&encrypt(self.0.as_slice(), aad, plaintext)?)
            .map_err(|_| CryptoError::EncryptionFailed)
    }
    pub fn open(&self, aad: &[u8], encoded: &str) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        if encoded.len() > MAX_ENVELOPE {
            return Err(CryptoError::InvalidEnvelope);
        }
        decrypt(
            self.0.as_slice(),
            aad,
            serde_json::from_str(encoded).map_err(|_| CryptoError::InvalidEnvelope)?,
        )
    }
    pub fn seal_api(&self, context: &str, plaintext: &str) -> Result<String, CryptoError> {
        self.seal(
            format!("zuse-cloud-api-v1\n{context}").as_bytes(),
            plaintext.as_bytes(),
        )
    }
    pub fn open_api(
        &self,
        context: &str,
        ciphertext: &str,
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        self.open(
            format!("zuse-cloud-api-v1\n{context}").as_bytes(),
            ciphertext,
        )
    }
    pub fn digest_api(&self, context: &str, plaintext: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.0.as_slice())
            .expect("HMAC accepts a 32-byte key");
        mac.update(format!("zuse-cloud-api-digest-v1\n{context}\n{plaintext}").as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn create_transcript_key(
        &self,
        owner: &str,
        workspace: &str,
    ) -> Result<(Zeroizing<String>, String), CryptoError> {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(key.as_mut());
        let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(key.as_slice()));
        let envelope = self.seal(
            format!("zuse-cloud-transcript-wrapping-v1\n{owner}\n{workspace}").as_bytes(),
            encoded.as_bytes(),
        )?;
        Ok((encoded, envelope))
    }
    pub fn open_transcript_key(
        &self,
        owner: &str,
        workspace: &str,
        envelope: &str,
    ) -> Result<Zeroizing<String>, CryptoError> {
        let bytes = self.open(
            format!("zuse-cloud-transcript-wrapping-v1\n{owner}\n{workspace}").as_bytes(),
            envelope,
        )?;
        let key = String::from_utf8(bytes.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::InvalidEnvelope)?;
        if decode(&key)?.len() != 32 {
            return Err(CryptoError::InvalidKey);
        }
        Ok(key)
    }
}

#[derive(Deserialize)]
struct RsaJwk {
    kty: String,
    n: String,
    e: String,
}

fn public_key(encoded: &str) -> Result<(RsaJwk, RsaPublicKey), CryptoError> {
    if encoded.len() > 16384 {
        return Err(CryptoError::InvalidKey);
    }
    let value: serde_json::Value =
        serde_json::from_str(encoded).map_err(|_| CryptoError::InvalidKey)?;
    if ["d", "p", "q", "dp", "dq", "qi"]
        .iter()
        .any(|name| value.get(name).is_some())
    {
        return Err(CryptoError::InvalidKey);
    }
    let jwk: RsaJwk = serde_json::from_value(value).map_err(|_| CryptoError::InvalidKey)?;
    if jwk.kty != "RSA" {
        return Err(CryptoError::InvalidKey);
    }
    let n = URL_SAFE_NO_PAD
        .decode(&jwk.n)
        .map_err(|_| CryptoError::InvalidKey)?;
    let e = URL_SAFE_NO_PAD
        .decode(&jwk.e)
        .map_err(|_| CryptoError::InvalidKey)?;
    let key = RsaPublicKey::new(BigUint::from_bytes_be(&n), BigUint::from_bytes_be(&e))
        .map_err(|_| CryptoError::InvalidKey)?;
    if !(2048..=8192).contains(&key.n().bits()) {
        return Err(CryptoError::InvalidKey);
    }
    Ok((jwk, key))
}

/// Public-key delivery for transcript/provider grants. The compact format matches jose.
pub fn seal_runtime_secret(public_jwk: &str, secret: &str) -> Result<String, CryptoError> {
    if secret.len() > 1024 * 1024 {
        return Err(CryptoError::InvalidEnvelope);
    }
    let (_, key) = public_key(public_jwk)?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RSA-OAEP-256","enc":"A256GCM"}"#);
    let mut cek = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(cek.as_mut());
    let wrapped = key
        .encrypt(&mut OsRng, Oaep::new::<Sha256>(), cek.as_slice())
        .map_err(|_| CryptoError::EncryptionFailed)?;
    let envelope = encrypt(cek.as_slice(), header.as_bytes(), secret.as_bytes())?;
    let ciphertext = decode(&envelope.ciphertext)?;
    let (body, tag) = ciphertext.split_at(ciphertext.len() - 16);
    Ok(format!(
        "{header}.{}.{}.{}.{}",
        URL_SAFE_NO_PAD.encode(wrapped),
        envelope.iv,
        URL_SAFE_NO_PAD.encode(body),
        URL_SAFE_NO_PAD.encode(tag)
    ))
}

/// RFC 7638 canonical RSA identity used by bootstrap receipts.
pub fn credential_thumbprint(public_jwk: &str) -> Result<String, CryptoError> {
    let (jwk, _) = public_key(public_jwk)?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!(r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#, jwk.e, jwk.n).as_bytes(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey};

    #[test]
    fn existing_webcrypto_envelopes_digests_and_runtime_jwe_are_compatible() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/cloud-crypto.json")).unwrap();
        let field = |name: &str| fixture[name].as_str().unwrap();
        let master = MasterKey::parse(field("masterKey")).unwrap();
        let plaintext = master
            .open_api(field("context"), field("apiEnvelope"))
            .unwrap();
        assert_eq!(plaintext.as_slice(), field("plaintext").as_bytes());
        assert_eq!(
            master
                .open_api("message\nother-owner", field("apiEnvelope"))
                .unwrap_err(),
            CryptoError::AuthenticationFailed
        );
        assert_eq!(
            master.digest_api("workspace-archive", field("archivePayload")),
            field("archiveDigest")
        );
        assert_eq!(
            master
                .open_transcript_key(
                    "user_fixture",
                    "workspace_fixture",
                    field("transcriptEnvelope")
                )
                .unwrap()
                .as_str(),
            field("transcriptKey")
        );
        assert!(
            master
                .open_transcript_key(
                    "user_fixture",
                    "other-workspace",
                    field("transcriptEnvelope")
                )
                .is_err()
        );
        let (key, envelope) = master
            .create_transcript_key("user_fixture", "workspace_fixture")
            .unwrap();
        assert_eq!(
            master
                .open_transcript_key("user_fixture", "workspace_fixture", &envelope)
                .unwrap()
                .as_str(),
            key.as_str()
        );
        let api = master
            .seal_api(field("context"), field("plaintext"))
            .unwrap();
        assert_ne!(api, field("apiEnvelope"));
        assert_eq!(
            master.open_api(field("context"), &api).unwrap().as_slice(),
            field("plaintext").as_bytes()
        );

        let public = fixture["runtimePublicJwk"].to_string();
        assert_eq!(
            credential_thumbprint(&public).unwrap(),
            field("runtimeThumbprint")
        );
        let mut private_input = fixture["runtimePublicJwk"].clone();
        private_input["d"] = serde_json::json!("private-material-must-not-be-admitted");
        assert_eq!(
            credential_thumbprint(&private_input.to_string()),
            Err(CryptoError::InvalidKey)
        );
        let jwe = seal_runtime_secret(&public, field("transcriptKey")).unwrap();
        let parts = jwe.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 5);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&decode(parts[0]).unwrap()).unwrap(),
            serde_json::json!({"alg":"RSA-OAEP-256","enc":"A256GCM"})
        );
        let private =
            RsaPrivateKey::from_pkcs8_der(&decode(field("runtimePrivatePkcs8")).unwrap()).unwrap();
        let cek = Zeroizing::new(
            private
                .decrypt(Oaep::new::<Sha256>(), &decode(parts[1]).unwrap())
                .unwrap(),
        );
        assert_eq!(cek.len(), 32);
        let mut ciphertext = decode(parts[3]).unwrap();
        ciphertext.extend(decode(parts[4]).unwrap());
        let plaintext = decrypt(
            &cek,
            parts[0].as_bytes(),
            Envelope {
                version: 1,
                key_id: None,
                iv: parts[2].into(),
                ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
            },
        )
        .unwrap();
        assert_eq!(plaintext.as_slice(), field("transcriptKey").as_bytes());
        assert_eq!(
            MasterKey::parse("not-a-key").err(),
            Some(CryptoError::InvalidKey)
        );
    }
}
