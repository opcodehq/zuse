//! Cryptographic verification for an account's one-use desktop link challenge.
use crate::error::ApiError;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde_json::Value;

pub fn verify(
    proof: &str,
    public_jwk: &Value,
    environment: &str,
    challenge: &str,
    issuer: &str,
    now: i64,
) -> Result<(), ApiError> {
    let rejected = || ApiError::unauthorized("invalid_environment_link_proof");
    if proof.len() > 16384
        || environment.is_empty()
        || environment.len() > 256
        || challenge.is_empty()
        || challenge.len() > 4096
    {
        return Err(rejected());
    }
    crate::tickets::signing_thumbprint(&public_jwk.to_string()).map_err(|_| rejected())?;
    let key =
        DecodingKey::from_jwk(&serde_json::from_value(public_jwk.clone()).map_err(|_| rejected())?)
            .map_err(|_| rejected())?;
    let mut validation = Validation::new(Algorithm::EdDSA);
    validation.set_audience(&[issuer]);
    validation.validate_exp = false;
    let decoded = decode::<Value>(proof, &key, &validation).map_err(|_| rejected())?;
    let claims = decoded.claims;
    let issued = claims["iat"].as_i64().ok_or_else(rejected)?;
    let expiry = claims["exp"].as_i64().ok_or_else(rejected)?;
    if decoded.header.typ.as_deref() != Some("environment-link-proof+jwt")
        || claims["environmentId"] != environment
        || claims["challenge"] != challenge
        || issued > now / 1000 + 30
        || issued < now / 1000 - 300
        || expiry <= now / 1000
        || expiry <= issued
        || expiry.saturating_sub(issued) > 300
        || claims
            .get("nbf")
            .is_some_and(|nbf| nbf.as_i64().is_none_or(|nbf| nbf > now / 1000))
    {
        return Err(rejected());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    #[test]
    fn desktop_link_proof_binds_challenge_host_audience_and_lifetime() {
        let key = SigningKey::from_bytes(&[17; 32]);
        let public = json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(key.verifying_key().as_bytes())});
        let der = key.to_pkcs8_der().unwrap();
        let encoding = EncodingKey::from_ed_der(der.as_bytes());
        let issuer = "http://127.0.0.1:28700";
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("environment-link-proof+jwt".into());
        let claims = json!({"aud":issuer,"environmentId":"desktop","challenge":"one-use","iat":1000,"exp":1300});
        let token = encode(&header, &claims, &encoding).unwrap();
        assert!(verify(&token, &public, "desktop", "one-use", issuer, 1_000_000).is_ok());
        assert!(verify(&token, &public, "other", "one-use", issuer, 1_000_000).is_err());
        assert!(verify(&token, &public, "desktop", "other", issuer, 1_000_000).is_err());
        assert!(
            verify(
                &token,
                &public,
                "desktop",
                "one-use",
                "https://other.example",
                1_000_000
            )
            .is_err()
        );
        assert!(verify(&token, &public, "desktop", "one-use", issuer, 1_300_000).is_err());
        assert!(verify(&token, &public, "desktop", "one-use", issuer, 900_000).is_err());
        let other = SigningKey::from_bytes(&[18; 32]);
        let other_public = json!({"kty":"OKP","crv":"Ed25519","x":URL_SAFE_NO_PAD.encode(other.verifying_key().as_bytes())});
        assert!(
            verify(
                &token,
                &other_public,
                "desktop",
                "one-use",
                issuer,
                1_000_000
            )
            .is_err()
        );
    }
}
