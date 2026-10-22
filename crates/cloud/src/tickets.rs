//! Existing Ed25519 workspace gateway ticket and runtime public-key identities.
use crate::error::ApiError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{SigningKey, VerifyingKey, pkcs8::EncodePrivateKey};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const CLIENT_TTL_MS: i64 = 60 * 1000;
pub const RUNTIME_TTL_MS: i64 = 15 * 60 * 1000;
pub const GATEWAY_PROTOCOL: &str = "zuse-workspace-v2";

/// The platform signing key is never serialized or logged.
pub struct TicketSigner {
    key: EncodingKey,
    verify: jsonwebtoken::DecodingKey,
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
            verify: jsonwebtoken::DecodingKey::from_jwk(
                &serde_json::from_value(json!({"kty":"OKP","crv":"Ed25519","x":expected}))
                    .map_err(|_| ApiError::bad_request("invalid_signing_key"))?,
            )
            .map_err(|_| ApiError::bad_request("invalid_signing_key"))?,
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

    pub fn client(&self, mut claims: Value, workspace: &str, now: i64) -> Result<String, ApiError> {
        claims["iss"] = json!(self.issuer());
        claims["aud"] = json!(format!("zuse-workspace:{workspace}"));
        claims["iat"] = json!(now / 1000);
        claims["exp"] = json!((now + CLIENT_TTL_MS) / 1000);
        claims["scope"] = json!("workspace-client");
        claims["role"] = json!("client");
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("workspace-client+jwt".into());
        jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|_| ApiError::unavailable("workspace_ticket_sign_failed"))
    }

    pub fn verify_gateway(
        &self,
        credential: &str,
        workspace: &str,
        protocol: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let rejected = || ApiError::unauthorized("workspace_gateway_rejected");
        if credential.len() > 16384 {
            return Err(rejected());
        }
        let mut validation = jsonwebtoken::Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.issuer()]);
        validation.set_audience(&[format!("zuse-workspace:{workspace}")]);
        validation.validate_exp = false;
        let decoded = jsonwebtoken::decode::<Value>(credential, &self.verify, &validation)
            .map_err(|_| rejected())?;
        let claims = decoded.claims;
        let valid_role = match claims["role"].as_str() {
            Some("runtime") => {
                decoded.header.typ.as_deref() == Some("workspace-runtime+jwt")
                    && claims["scope"] == "workspace-runtime"
                    && [
                        GATEWAY_PROTOCOL,
                        zuse_protocol::cloud_gateway::LEGACY_PROTOCOL,
                    ]
                    .contains(&protocol)
            }
            Some("client") => {
                decoded.header.typ.as_deref() == Some("workspace-client+jwt")
                    && claims["scope"] == "workspace-client"
                    && claims["deviceId"].as_str().is_some_and(|d| !d.is_empty())
                    && (claims.get("actorId").is_none()
                        || claims["actorId"].as_str().is_some_and(|a| !a.is_empty()))
                    && (claims.get("permission").is_none()
                        || matches!(claims["permission"].as_str(), Some("view" | "edit")))
            }
            _ => false,
        };
        if !valid_role
            || claims["workspaceId"] != workspace
            || claims["protocol"] != protocol
            || claims["exp"].as_i64().is_none_or(|exp| exp <= now / 1000)
            || claims
                .get("nbf")
                .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now / 1000))
            || claims["sub"].as_str().is_none_or(|s| s.is_empty())
        {
            return Err(rejected());
        }
        Ok(claims)
    }

    pub fn issuer(&self) -> String {
        self.issuer.origin().ascii_serialization()
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

/// Validate the registered guest's fresh, workspace-bound proof even after its bearer expires.
pub fn verify_renewal_proof(
    proof: &str,
    public_jwk: &str,
    issuer: &str,
    workspace: &str,
    request: &str,
    identity: (i64, i64),
    now: i64,
) -> Result<(), ApiError> {
    use jsonwebtoken::{DecodingKey, Validation, decode};
    let rejected = || ApiError::unauthorized("invalid_runtime_renewal_proof");
    if proof.len() > 16384 {
        return Err(rejected());
    }
    let jwk = serde_json::from_str(public_jwk).map_err(|_| rejected())?;
    let key = DecodingKey::from_jwk(&jwk).map_err(|_| rejected())?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_audience(&[issuer]);
    // Lifecycle code supplies the transaction's time; evaluate JWT time claims against it.
    validation.validate_exp = false;
    let decoded = decode::<Value>(proof, &key, &validation).map_err(|_| rejected())?;
    let claims = decoded.claims;
    if claims["workspaceId"] == workspace
        && claims["requestId"] == request
        && (claims["generation"] != identity.0 || claims["gatewayEpoch"] != identity.1)
    {
        return Err(ApiError::unauthorized("workspace_runtime_fenced"));
    }
    if decoded.header.typ.as_deref() != Some("workspace-runtime-renewal+jwt")
        || claims["workspaceId"] != workspace
        || claims["requestId"] != request
        || claims["generation"] != identity.0
        || claims["gatewayEpoch"] != identity.1
        || claims["iat"]
            .as_i64()
            .is_none_or(|iat| iat > now / 1000 + 30 || iat < now / 1000 - 120)
        || claims["exp"].as_i64().is_none_or(|exp| exp <= now / 1000)
        || claims
            .get("nbf")
            .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now / 1000))
    {
        return Err(ApiError::unauthorized(
            "runtime_renewal_proof_binding_mismatch",
        ));
    }
    Ok(())
}
