//! Existing Ed25519 workspace gateway ticket and runtime public-key identities.
use crate::error::ApiError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{SigningKey, VerifyingKey, pkcs8::EncodePrivateKey};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const RUNTIME_TTL_MS: i64 = 15 * 60 * 1000;
pub const GATEWAY_PROTOCOL: &str = "zuse-workspace-v2";

/// The platform signing key is never serialized or logged.
pub struct TicketSigner {
    key: EncodingKey,
    issuer: reqwest::Url,
}

impl TicketSigner {
    pub fn from_jwk(private_jwk: &str, issuer: &str) -> Result<Self, ApiError> {
        if private_jwk.len() > 4096 {
            return Err(ApiError::bad_request("invalid_signing_key"));
        }
        let value: Value = serde_json::from_str(private_jwk)
            .map_err(|_| ApiError::bad_request("invalid_signing_key"))?;
        let raw = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(value["d"].as_str().unwrap_or_default())
                .map_err(|_| ApiError::bad_request("invalid_signing_key"))?,
        );
        let bytes: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| ApiError::bad_request("invalid_signing_key"))?;
        let signing = SigningKey::from_bytes(&bytes);
        let expected = URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes());
        if value["kty"] != "OKP" || value["crv"] != "Ed25519" || value["x"] != expected {
            return Err(ApiError::bad_request("invalid_signing_key"));
        }
        let der = signing
            .to_pkcs8_der()
            .map_err(|_| ApiError::bad_request("invalid_signing_key"))?;
        let issuer =
            reqwest::Url::parse(issuer).map_err(|_| ApiError::bad_request("invalid_api_issuer"))?;
        if issuer.scheme() != "https"
            || !issuer.username().is_empty()
            || issuer.password().is_some()
            || issuer.query().is_some()
            || issuer.fragment().is_some()
            || issuer.path() != "/"
        {
            return Err(ApiError::bad_request("invalid_api_issuer"));
        }
        Ok(Self {
            key: EncodingKey::from_ed_der(der.as_bytes()),
            issuer,
        })
    }

    pub fn runtime(
        &self,
        owner: &str,
        workspace: &str,
        generation: i64,
        epoch: i64,
        expires: i64,
    ) -> Result<String, ApiError> {
        if generation < 1 || epoch < 1 || expires < RUNTIME_TTL_MS {
            return Err(ApiError::bad_request("invalid_runtime_generation"));
        }
        let claims = json!({"workspaceId":workspace,"scope":"workspace-runtime","role":"runtime", "protocol":GATEWAY_PROTOCOL,
            "generation":generation,"gatewayEpoch":epoch,"iss":self.issuer.origin().ascii_serialization(),
            "aud":format!("zuse-workspace:{workspace}"),"sub":owner,"iat":(expires-RUNTIME_TTL_MS)/1000,"exp":expires/1000});
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("workspace-runtime+jwt".into());
        jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|_| ApiError::unavailable("workspace_runtime_ticket_sign_failed"))
    }

    pub fn gateway_url(&self, workspace: &str) -> String {
        let mut url = self.issuer.clone();
        url.set_scheme("wss").expect("HTTPS issuer can use WSS");
        url.path_segments_mut()
            .expect("HTTPS origin has path segments")
            .clear()
            .extend(["v1", "cloud", "workspaces", workspace, "gateway"]);
        url.into()
    }
}

pub fn signing_thumbprint(encoded: &str) -> Result<String, ApiError> {
    if encoded.len() > 4096 {
        return Err(ApiError::bad_request("invalid_runtime_signing_key"));
    }
    let value: Value = serde_json::from_str(encoded)
        .map_err(|_| ApiError::bad_request("invalid_runtime_signing_key"))?;
    let x = value["x"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("invalid_runtime_signing_key"))?;
    let raw = URL_SAFE_NO_PAD
        .decode(x)
        .map_err(|_| ApiError::bad_request("invalid_runtime_signing_key"))?;
    let bytes: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::bad_request("invalid_runtime_signing_key"))?;
    if value["kty"] != "OKP"
        || value["crv"] != "Ed25519"
        || value.get("d").is_some()
        || VerifyingKey::from_bytes(&bytes).is_err()
    {
        return Err(ApiError::bad_request("invalid_runtime_signing_key"));
    }
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(
        format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#).as_bytes(),
    )))
}
