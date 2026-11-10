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
    pub fn device_connection_token(
        &self,
        account: &str,
        environment: &str,
        thumbprint: &str,
        now: i64,
    ) -> Result<String, ApiError> {
        if account.is_empty()
            || account.len() > 256
            || environment.is_empty()
            || environment.len() > 256
            || thumbprint.len() != 43
        {
            return Err(ApiError::bad_request("invalid_connection_identity"));
        }
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("connect+jwt".into());
        jsonwebtoken::encode(&header,&json!({"iss":self.issuer(),"aud":format!("zuse-env:{environment}"),"sub":account,"environmentId":environment,"cnf":{"jkt":thumbprint},"iat":now/1000,"exp":now/1000+60}),&self.key)
            .map_err(|_| ApiError::unavailable("connection_token_sign_failed"))
    }

    pub fn device_access_token(
        &self,
        account: &str,
        thumbprint: &str,
        now: i64,
    ) -> Result<String, ApiError> {
        if account.is_empty() || account.len() > 256 || thumbprint.len() != 43 {
            return Err(ApiError::bad_request("invalid_device_identity"));
        }
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("at+jwt".into());
        jsonwebtoken::encode(
            &header,
            &json!({
                "iss":self.issuer(),"sub":account,"iat":now/1000,"exp":now/1000+300,
                "scope":"status connect register","cnf":{"jkt":thumbprint}
            }),
            &self.key,
        )
        .map_err(|_| ApiError::unavailable("device_token_sign_failed"))
    }

    pub fn verify_device_access(
        &self,
        token: &str,
        scope: &str,
        now: i64,
    ) -> Result<(String, String), ApiError> {
        use jsonwebtoken::{Validation, decode};
        let rejected = || ApiError::unauthorized("invalid_device_access_token");
        if token.len() > 16384 {
            return Err(rejected());
        }
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.issuer()]);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let decoded = decode::<Value>(token, &self.verify, &validation).map_err(|_| rejected())?;
        let c = decoded.claims;
        let account = c["sub"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .ok_or_else(rejected)?;
        let thumbprint = c["cnf"]["jkt"]
            .as_str()
            .filter(|s| s.len() == 43)
            .ok_or_else(rejected)?;
        let issued = c["iat"].as_i64().ok_or_else(rejected)?;
        let expiry = c["exp"].as_i64().ok_or_else(rejected)?;
        if decoded.header.typ.as_deref() != Some("at+jwt")
            || expiry <= now / 1000
            || issued > now / 1000 + 30
            || expiry <= issued
            || expiry.saturating_sub(issued) > 300
            || !c["scope"]
                .as_str()
                .is_some_and(|s| s.split_whitespace().any(|item| item == scope))
        {
            return Err(rejected());
        }
        Ok((account.into(), thumbprint.into()))
    }

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
        let local_http = issuer.scheme() == "http"
            && issuer.host_str().is_some_and(|h| {
                h == "localhost"
                    || h.parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
        if (issuer.scheme() != "https" && !local_http)
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
        self.runtime_with_rpc(
            owner,
            workspace,
            generation,
            epoch,
            expires,
            zuse_protocol::cloud_gateway::LEGACY_RPC,
        )
    }
    pub fn runtime_with_rpc(
        &self,
        owner: &str,
        workspace: &str,
        generation: i64,
        epoch: i64,
        expires: i64,
        rpc_protocol: &str,
    ) -> Result<String, ApiError> {
        if generation < 1 || epoch < 1 || expires < RUNTIME_TTL_MS {
            return Err(ApiError::bad_request("invalid_runtime_generation"));
        }
        let protocol = if rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC {
            zuse_protocol::cloud_gateway::RUST_PROTOCOL
        } else {
            GATEWAY_PROTOCOL
        };
        let mut claims = json!({"workspaceId":workspace,"scope":"workspace-runtime","role":"runtime", "protocol":protocol,
            "generation":generation,"gatewayEpoch":epoch,"iss":self.issuer.origin().ascii_serialization(),
            "aud":format!("zuse-workspace:{workspace}"),"sub":owner,"iat":(expires-RUNTIME_TTL_MS)/1000,"exp":expires/1000});
        if rpc_protocol != zuse_protocol::cloud_gateway::LEGACY_RPC {
            claims["rpcProtocol"] = json!(rpc_protocol);
        }
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
                        zuse_protocol::cloud_gateway::RUST_PROTOCOL,
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

    pub fn checkout(&self, owner: &str, offer: &str, now: i64) -> Result<String, ApiError> {
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("checkout-receipt+jwt".into());
        jsonwebtoken::encode(&header, &json!({"iss":self.issuer(),"aud":"zuse-checkout-receipt","sub":owner,"offerId":offer,"iat":now/1000,"exp":(now+86400000)/1000}), &self.key)
            .map_err(|_|ApiError::unavailable("checkout_ticket_sign_failed"))
    }
    pub fn verify_checkout(&self, token: &str, now: i64) -> Result<Value, ApiError> {
        let invalid = || ApiError::unauthorized("invalid_checkout_ticket");
        if token.len() > 16384 {
            return Err(invalid());
        }
        let mut validation = jsonwebtoken::Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.issuer()]);
        validation.set_audience(&["zuse-checkout-receipt"]);
        validation.validate_exp = false;
        let decoded = jsonwebtoken::decode::<Value>(token, &self.verify, &validation)
            .map_err(|_| invalid())?;
        if decoded.header.typ.as_deref() != Some("checkout-receipt+jwt")
            || decoded.claims["exp"]
                .as_i64()
                .is_none_or(|v| v <= now / 1000)
            || decoded.claims["iat"]
                .as_i64()
                .is_none_or(|v| v > now / 1000)
            || decoded.claims["sub"].as_str().is_none_or(|s| s.is_empty())
            || decoded.claims["offerId"]
                .as_str()
                .is_none_or(|s| s.is_empty())
        {
            return Err(invalid());
        }
        Ok(decoded.claims)
    }

    pub fn issuer(&self) -> String {
        self.issuer.origin().ascii_serialization()
    }

    pub fn gateway_url(&self, workspace: &str) -> String {
        let mut url = self.issuer.clone();
        url.set_scheme(if self.issuer.scheme() == "https" {
            "wss"
        } else {
            "ws"
        })
        .expect("HTTP issuer can use WebSocket");
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

#[cfg(test)]
mod issuer_tests {
    use super::*;
    #[test]
    fn device_access_tokens_are_scoped_expiring_and_bound_to_the_mint() {
        fn signer(seed: u8) -> TicketSigner {
            let key = SigningKey::from_bytes(&[seed; 32]);
            let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([seed;32]),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())});
            TicketSigner::from_jwk(&jwk.to_string(), "http://127.0.0.1:28700").unwrap()
        }
        let mint = signer(19);
        let thumbprint = "a".repeat(43);
        let token = mint
            .device_access_token("user-test", &thumbprint, 1_000_000)
            .unwrap();
        assert_eq!(
            mint.verify_device_access(&token, "register", 1_001_000)
                .unwrap(),
            ("user-test".into(), thumbprint)
        );
        assert!(
            mint.verify_device_access(&token, "admin", 1_001_000)
                .is_err()
        );
        assert!(
            mint.verify_device_access(&token, "register", 1_300_000)
                .is_err()
        );
        assert!(
            mint.verify_device_access(&token, "register", 900_000)
                .is_err()
        );
        assert!(
            signer(20)
                .verify_device_access(&token, "register", 1_001_000)
                .is_err()
        );
    }
    #[test]
    fn plaintext_gateway_issuers_are_limited_to_loopback() {
        let seed = [21u8; 32];
        let key = SigningKey::from_bytes(&seed);
        let jwk=json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode(seed),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())}).to_string();
        let local = TicketSigner::from_jwk(&jwk, "http://127.0.0.1:8080").unwrap();
        assert_eq!(
            local.gateway_url("workspace"),
            "ws://127.0.0.1:8080/v1/cloud/workspaces/workspace/gateway"
        );
        for invalid in [
            "http://api-rust.zuse.sh",
            "http://0.0.0.0:8080",
            "https://user:password@api-rust.zuse.sh",
            "https://api-rust.zuse.sh/path",
        ] {
            assert!(TicketSigner::from_jwk(&jwk, invalid).is_err());
        }
        let production = TicketSigner::from_jwk(&jwk, "https://api-rust.zuse.sh").unwrap();
        assert!(production.gateway_url("workspace").starts_with("wss://"));
    }
}

impl TicketSigner {
    /// Internal short-lived proof; never accepted as a client/runtime ticket.
    pub(crate) fn allocation_scope_proof(
        &self,
        workspace: &str,
        operation: &str,
        generation: i64,
        now: i64,
    ) -> Result<String, ApiError> {
        let claims = json!({"iss":self.issuer(),"aud":"zuse-allocation-scope","scope":"allocation-scope","workspaceId":workspace,"operationId":operation,"generation":generation,"iat":now/1000,"exp":(now+10000)/1000});
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("zuse-allocation-scope+jwt".into());
        jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|_| ApiError::unavailable("allocation_scope_sign_failed"))
    }
    pub(crate) fn verify_allocation_scope(
        &self,
        proof: &str,
        workspace: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let invalid = || ApiError::unauthorized("allocation_scope_proof_rejected");
        if proof.len() > 16384 {
            return Err(invalid());
        }
        let mut validation = jsonwebtoken::Validation::new(Algorithm::EdDSA);
        validation.set_audience(&["zuse-allocation-scope"]);
        validation.set_issuer(&[self.issuer()]);
        validation.validate_exp = false;
        let decoded = jsonwebtoken::decode::<Value>(proof, &self.verify, &validation)
            .map_err(|_| invalid())?;
        let claims = decoded.claims;
        if decoded.header.typ.as_deref() != Some("zuse-allocation-scope+jwt")
            || claims["scope"] != "allocation-scope"
            || claims["workspaceId"] != workspace
            || claims["exp"].as_i64().is_none_or(|t| t * 1000 <= now)
            || claims["iat"]
                .as_i64()
                .is_none_or(|t| t * 1000 > now + 1000 || t * 1000 < now - 10000)
            || claims["generation"].as_i64().is_none_or(|g| g < 1)
        {
            return Err(invalid());
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod allocation_scope_tests {
    use super::*;
    #[test]
    fn worker_allocation_scope_proof_is_short_lived_and_not_client_authority() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[99; 32]);
        let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([99;32]),"x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())});
        let signer = TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test").unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let proof = signer
            .allocation_scope_proof("task", "operation", 7, now)
            .unwrap();
        let claims = signer.verify_allocation_scope(&proof, "task", now).unwrap();
        assert_eq!(claims["operationId"], "operation");
        assert_eq!(claims["generation"], 7);
        assert!(
            signer
                .verify_allocation_scope(&proof, "other", now)
                .is_err()
        );
        assert!(
            signer
                .verify_allocation_scope(&proof, "task", now + 11000)
                .is_err()
        );
        assert!(
            signer
                .verify_gateway(
                    &proof,
                    "task",
                    zuse_protocol::cloud_gateway::RUST_PROTOCOL,
                    now
                )
                .is_err()
        );
    }
}
