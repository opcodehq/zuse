//! The bridge's own proof key: one P-256 key per profile, signing a
//! single-use ES256 proof for each request (method, URL and access token).
//! Independent of remote access.
use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::{ecdsa::SigningKey, pkcs8::EncodePrivateKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(crate) struct ProofKey {
    key: EncodingKey,
    jwk: Value,
}

impl ProofKey {
    pub(crate) fn new(secret: &SigningKey) -> Result<Self> {
        let point = secret.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty":"EC","crv":"P-256",
            "x":URL_SAFE_NO_PAD.encode(point.x().context("Invalid computer bridge key")?),
            "y":URL_SAFE_NO_PAD.encode(point.y().context("Invalid computer bridge key")?)});
        let der = secret
            .to_pkcs8_der()
            .ok()
            .context("Invalid computer bridge key")?;
        Ok(Self {
            key: EncodingKey::from_ec_der(der.as_bytes()),
            jwk,
        })
    }

    pub(crate) fn sign(&self, method: &str, url: &str, access: &str) -> Result<String> {
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".into());
        header.jwk = Some(serde_json::from_value(self.jwk.clone())?);
        let payload = json!({"htm":method,"htu":url,"jti":uuid::Uuid::new_v4().to_string(),
            "iat":chrono::Utc::now().timestamp(),
            "ath":URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes()))});
        Ok(jsonwebtoken::encode(&header, &payload, &self.key)?)
    }
}
