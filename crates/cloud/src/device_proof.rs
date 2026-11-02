//! Native ES256 DPoP verification. Replay consumption must follow verification.
use crate::error::ApiError;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct VerifiedProof {
    pub public_jwk: Value,
    pub thumbprint: String,
    pub jti: String,
    pub issued_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use p256::{ecdsa::SigningKey, pkcs8::EncodePrivateKey};
    use serde_json::json;

    #[test]
    fn signed_native_proof_binds_request_time_and_access_token() {
        let secret = SigningKey::from_slice(&[7; 32]).unwrap();
        let point = secret.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(point.x().unwrap()),"y":URL_SAFE_NO_PAD.encode(point.y().unwrap())});
        let der = secret.to_pkcs8_der().unwrap();
        let key = EncodingKey::from_ec_der(der.as_bytes());
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("dpop+jwt".into());
        header.jwk = Some(serde_json::from_value(jwk).unwrap());
        let now = 1_000_000;
        let target = "http://127.0.0.1:28700/v1/mobile/devices";
        let claims = json!({"htm":"POST","htu":target,"iat":1000,"jti":"native-proof","ath":URL_SAFE_NO_PAD.encode(Sha256::digest(b"access-token"))});
        let proof = encode(&header, &claims, &key).unwrap();
        let verified = verify(&proof, "POST", target, Some("access-token"), now).unwrap();
        assert_eq!(verified.jti, "native-proof");
        assert_eq!(verified.issued_at, now);
        assert_eq!(verified.thumbprint.len(), 43);
        for (method, url, access, time) in [
            ("GET", target, "access-token", now),
            ("POST", "http://127.0.0.1:28700/other", "access-token", now),
            ("POST", target, "other-token", now),
            ("POST", target, "access-token", now + 121_000),
            ("POST", target, "access-token", now - 31_000),
        ] {
            assert!(verify(&proof, method, url, Some(access), time).is_err());
        }
        let mut missing_hash = claims.clone();
        missing_hash.as_object_mut().unwrap().remove("ath");
        let missing = encode(&header, &missing_hash, &key).unwrap();
        assert!(verify(&missing, "POST", target, Some("access-token"), now).is_err());
        assert!(verify(&missing, "POST", target, None, now).is_ok());
        let mut tampered: Vec<_> = proof.split('.').map(str::to_owned).collect();
        let mut signature = URL_SAFE_NO_PAD.decode(&tampered[2]).unwrap();
        signature[0] ^= 1;
        tampered[2] = URL_SAFE_NO_PAD.encode(signature);
        assert!(
            verify(
                &tampered.join("."),
                "POST",
                target,
                Some("access-token"),
                now
            )
            .is_err()
        );
        header.typ = Some("JWT".into());
        let wrong_type = encode(&header, &claims, &key).unwrap();
        assert!(verify(&wrong_type, "POST", target, Some("access-token"), now).is_err());
    }
}

pub fn verify(
    proof: &str,
    method: &str,
    target: &str,
    access: Option<&str>,
    now: i64,
) -> Result<VerifiedProof, ApiError> {
    let rejected = || ApiError::unauthorized("invalid_dpop_proof");
    if proof.len() > 16384 {
        return Err(rejected());
    }
    let raw = URL_SAFE_NO_PAD
        .decode(proof.split('.').next().ok_or_else(rejected)?)
        .map_err(|_| rejected())?;
    let raw: Value = serde_json::from_slice(&raw).map_err(|_| rejected())?;
    let jwk = &raw["jwk"];
    if raw.get("crit").is_some()
        || jwk.get("d").is_some()
        || jwk["kty"] != "EC"
        || jwk["crv"] != "P-256"
    {
        return Err(rejected());
    }
    let x = jwk["x"].as_str().ok_or_else(rejected)?;
    let y = jwk["y"].as_str().ok_or_else(rejected)?;
    for coordinate in [x, y] {
        let bytes = URL_SAFE_NO_PAD.decode(coordinate).map_err(|_| rejected())?;
        if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != coordinate {
            return Err(rejected());
        }
    }
    let header = decode_header(proof).map_err(|_| rejected())?;
    if header.alg != Algorithm::ES256 || header.typ.as_deref() != Some("dpop+jwt") {
        return Err(rejected());
    }
    let key =
        DecodingKey::from_jwk(header.jwk.as_ref().ok_or_else(rejected)?).map_err(|_| rejected())?;
    let mut validation = Validation::new(Algorithm::ES256);
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    validation.validate_aud = false;
    let claims = decode::<Value>(proof, &key, &validation)
        .map_err(|_| rejected())?
        .claims;
    let issued_at = claims["iat"]
        .as_i64()
        .and_then(|iat| iat.checked_mul(1000))
        .ok_or_else(rejected)?;
    let jti = claims["jti"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .ok_or_else(rejected)?;
    if claims["htm"] != method
        || claims["htu"] != target
        || issued_at < now.saturating_sub(120000)
        || issued_at > now.saturating_add(30000)
    {
        return Err(rejected());
    }
    if let Some(access) = access {
        if claims["ath"] != URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes())) {
            return Err(rejected());
        }
    }
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    Ok(VerifiedProof {
        public_jwk: serde_json::json!({"kty":"EC","crv":"P-256","x":x,"y":y}),
        thumbprint: URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes())),
        jti: jti.to_owned(),
        issued_at,
    })
}
