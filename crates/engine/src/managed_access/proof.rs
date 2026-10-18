//! One ephemeral proof key per account lifetime; never exported to views.
use super::{Error, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::{ecdsa::SigningKey, pkcs8::EncodePrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
pub(super) struct ProofKey {
    key: EncodingKey,
    pub jwk: Value,
}
impl ProofKey {
    pub fn new() -> Result<Self> {
        let secret = SigningKey::random(&mut rand_core::OsRng);
        let point = secret.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(point.x().ok_or(Error::Identity)?),"y":URL_SAFE_NO_PAD.encode(point.y().ok_or(Error::Identity)?)});
        let der = secret.to_pkcs8_der().map_err(|_| Error::Identity)?;
        Ok(Self {
            key: EncodingKey::from_ec_der(der.as_bytes()),
            jwk,
        })
    }
    pub fn sign(&self, method: &str, url: &str, access: Option<&str>) -> Result<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".into());
        header.jwk = Some(serde_json::from_value(self.jwk.clone()).map_err(|_| Error::Identity)?);
        let mut payload = json!({"htm":method,"htu":url,"jti":uuid::Uuid::new_v4().to_string(),"iat":chrono::Utc::now().timestamp()});
        if let Some(access) = access {
            payload["ath"] = json!(URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes())));
        }
        jsonwebtoken::encode(&header, &payload, &self.key).map_err(|_| Error::Identity)
    }
}
