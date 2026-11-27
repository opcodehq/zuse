//! Connector proofs: an ES256 proof-of-possession token signed by the
//! computer's own bridge key, bound to the method, path and access token, and
//! accepted once. The bridge owns this check and shares no code with remote
//! access.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use sha2::{Digest, Sha256};

/// A verified proof: the key's RFC 7638 thumbprint and the single-use ID.
#[derive(Debug)]
pub(super) struct Proof {
    pub thumbprint: String,
    pub jti: String,
}

/// Check a proof's signature and claims. `now_ms` is wall-clock milliseconds.
pub(super) fn verify(
    proof: &str,
    method: &str,
    path: &str,
    access: &str,
    now_ms: i64,
) -> Result<Proof, ApiError> {
    let rejected = || ApiError::unauthorized("invalid_bridge_proof");
    if proof.len() > 4096 {
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
        .filter(|id| !id.is_empty() && id.len() <= 128)
        .ok_or_else(rejected)?;
    let target = claims["htu"]
        .as_str()
        .and_then(|htu| reqwest::Url::parse(htu).ok())
        .ok_or_else(rejected)?;
    if claims["htm"] != method
        || target.path() != path
        || target.query().is_some()
        || !matches!(target.scheme(), "https" | "http")
        || issued_at < now_ms.saturating_sub(120_000)
        || issued_at > now_ms.saturating_add(30_000)
        || claims["ath"] != URL_SAFE_NO_PAD.encode(Sha256::digest(access.as_bytes()))
    {
        return Err(rejected());
    }
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    Ok(Proof {
        thumbprint: URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes())),
        jti: jti.to_owned(),
    })
}

/// Accept a verified proof once.
pub(super) async fn consume(pool: &sqlx::PgPool, proof: &Proof) -> Result<(), ApiError> {
    // Expired proofs are cleared a few at a time on the connector's own requests.
    sqlx::query(
        "DELETE FROM api_bridge_proofs WHERE (thumbprint,jti) IN
         (SELECT thumbprint,jti FROM api_bridge_proofs WHERE expires_at<now() LIMIT 64)",
    )
    .execute(pool)
    .await?;
    let fresh = sqlx::query(
        "INSERT INTO api_bridge_proofs(thumbprint,jti,expires_at) VALUES($1,$2,now()+interval '5 minutes')
         ON CONFLICT DO NOTHING",
    )
    .bind(&proof.thumbprint)
    .bind(&proof.jti)
    .execute(pool)
    .await?;
    if fresh.rows_affected() != 1 {
        return Err(ApiError::unauthorized("bridge_proof_replayed"));
    }
    Ok(())
}

/// The connector key behind a request: verified, then consumed.
pub(super) async fn host_key(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    path: &str,
) -> Result<String, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.is_empty())
        .map(|(_, t)| t)
        .ok_or_else(|| ApiError::unauthorized("missing_bearer"))?;
    let proof = headers
        .get("dpop")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("missing_bridge_proof"))?;
    let proof = verify(
        proof,
        "POST",
        path,
        token,
        chrono::Utc::now().timestamp_millis(),
    )?;
    consume(pool, &proof).await?;
    Ok(proof.thumbprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use p256::{ecdsa::SigningKey, pkcs8::EncodePrivateKey};

    fn sign(claims: &Value, typ: &str) -> String {
        let secret = SigningKey::from_slice(&[9; 32]).unwrap();
        let point = secret.verifying_key().to_encoded_point(false);
        let jwk = json!({"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(point.x().unwrap()),"y":URL_SAFE_NO_PAD.encode(point.y().unwrap())});
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(typ.into());
        header.jwk = Some(serde_json::from_value(jwk).unwrap());
        let der = secret.to_pkcs8_der().unwrap();
        encode(&header, claims, &EncodingKey::from_ec_der(der.as_bytes())).unwrap()
    }

    #[test]
    fn proofs_bind_method_path_time_and_access_token() {
        let now = 1_000_000;
        let claims = json!({"htm":"POST","htu":"https://api.example.test/v1/cloud/bridge/host-task",
            "iat":1000,"jti":"one","ath":URL_SAFE_NO_PAD.encode(Sha256::digest(b"token"))});
        let proof = sign(&claims, "dpop+jwt");
        let verified = verify(&proof, "POST", "/v1/cloud/bridge/host-task", "token", now).unwrap();
        assert_eq!(verified.jti, "one");
        assert_eq!(verified.thumbprint.len(), 43);
        for (method, path, token, at) in [
            ("GET", "/v1/cloud/bridge/host-task", "token", now),
            ("POST", "/v1/cloud/bridge/primary", "token", now),
            ("POST", "/v1/cloud/bridge/host-task", "other", now),
            ("POST", "/v1/cloud/bridge/host-task", "token", now + 121_000),
            ("POST", "/v1/cloud/bridge/host-task", "token", now - 31_000),
        ] {
            assert!(verify(&proof, method, path, token, at).is_err());
        }
        let mut unbound = claims.clone();
        unbound.as_object_mut().unwrap().remove("ath");
        assert!(
            verify(
                &sign(&unbound, "dpop+jwt"),
                "POST",
                "/v1/cloud/bridge/host-task",
                "token",
                now
            )
            .is_err()
        );
        assert!(
            verify(
                &sign(&claims, "JWT"),
                "POST",
                "/v1/cloud/bridge/host-task",
                "token",
                now
            )
            .is_err()
        );
        let mut parts: Vec<String> = proof.split('.').map(str::to_owned).collect();
        let mut signature = URL_SAFE_NO_PAD.decode(&parts[2]).unwrap();
        signature[0] ^= 1;
        parts[2] = URL_SAFE_NO_PAD.encode(signature);
        assert!(
            verify(
                &parts.join("."),
                "POST",
                "/v1/cloud/bridge/host-task",
                "token",
                now
            )
            .is_err()
        );
    }
}
