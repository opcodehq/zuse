//! Immutable R2 objects through the S3 API. Bodies and credentials never enter errors.
use crate::error::ApiError;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::{env, sync::Arc, time::Duration};

pub const MAX_OBJECT_BYTES: usize = 4 * 1024 * 1024;
#[async_trait]
pub trait Objects: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError>;
    /// An existing object must contain exactly the same bytes.
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError>;
}
pub struct R2 {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    bucket: String,
    access: String,
    secret: zeroize::Zeroizing<String>,
}
impl R2 {
    pub fn from_env() -> anyhow::Result<Option<Arc<dyn Objects>>> {
        let values = [
            "CLOUD_TRANSCRIPT_R2_ENDPOINT",
            "CLOUD_TRANSCRIPT_R2_BUCKET",
            "CLOUD_TRANSCRIPT_R2_ACCESS_KEY_ID",
            "CLOUD_TRANSCRIPT_R2_SECRET_ACCESS_KEY",
        ]
        .map(|k| env::var(k).ok());
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let [endpoint, bucket, access, secret] = values.map(|v| v.unwrap_or_default());
        Ok(Some(Arc::new(Self::new(
            &endpoint, &bucket, &access, &secret,
        )?)))
    }
    pub fn new(endpoint: &str, bucket: &str, access: &str, secret: &str) -> anyhow::Result<Self> {
        let url =
            reqwest::Url::parse(endpoint).map_err(|_| anyhow::anyhow!("Invalid R2 endpoint"))?;
        anyhow::ensure!(
            url.scheme() == "https"
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "R2 requires an HTTPS origin"
        );
        anyhow::ensure!(
            !bucket.is_empty()
                && bucket.len() <= 63
                && bucket.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b'-'
                    || b == b'.'),
            "Invalid R2 bucket"
        );
        anyhow::ensure!(
            !access.is_empty()
                && access.len() <= 256
                && access.bytes().all(|b| b.is_ascii_alphanumeric())
                && !secret.is_empty()
                && secret.len() <= 1024,
            "Invalid R2 credentials"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(20))
                .build()?,
            endpoint: url,
            bucket: bucket.into(),
            access: access.into(),
            secret: zeroize::Zeroizing::new(secret.into()),
        })
    }
    async fn request(
        &self,
        method: reqwest::Method,
        key: &str,
        body: Option<&[u8]>,
    ) -> Result<reqwest::Response, ApiError> {
        if key.is_empty()
            || key.len() > 2048
            || key.split('/').any(|s| s == "." || s == "..")
            || body.is_some_and(|b| b.len() > MAX_OBJECT_BYTES)
        {
            return Err(failed());
        }
        // Encode literal percent signs again: reference object names already contain encodeURIComponent segments.
        let path = format!("/{}/{}", self.bucket, encode(key, true));
        let mut url = self.endpoint.clone();
        url.set_path(&path);
        if url.path() != path {
            return Err(failed());
        }
        let host = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str().ok_or_else(failed)?),
            None => url.host_str().ok_or_else(failed)?.into(),
        };
        let date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let bytes = body.unwrap_or_default();
        let digest = hex(&Sha256::digest(bytes));
        let authorization = sign(
            &self.access,
            &self.secret,
            "auto",
            method.as_str(),
            &path,
            &host,
            &date,
            &digest,
            body.is_some(),
        );
        let mut authorization =
            reqwest::header::HeaderValue::from_str(&authorization).map_err(|_| failed())?;
        authorization.set_sensitive(true);
        let mut request = self
            .http
            .request(method, url)
            .header("x-amz-date", date)
            .header("x-amz-content-sha256", digest)
            .header("authorization", authorization);
        if let Some(bytes) = body {
            request = request.header("if-none-match", "*").body(bytes.to_vec());
        }
        request.send().await.map_err(|_| failed())
    }
}
fn failed() -> ApiError {
    ApiError::unavailable("cloud_transcript_store_unavailable")
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
/// S3 path encoding deliberately preserves slash and never normalizes object names.
fn encode(value: &str, slash: bool) -> String {
    value
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(b) || (slash && *b == b'/') {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn hmac(key: &[u8], body: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key");
    mac.update(body.as_bytes());
    mac.finalize().into_bytes().to_vec()
}
#[allow(clippy::too_many_arguments)]
fn sign(
    access: &str,
    secret: &str,
    region: &str,
    method: &str,
    path: &str,
    host: &str,
    date: &str,
    digest: &str,
    immutable: bool,
) -> String {
    let (headers, signed) = if immutable {
        (
            format!(
                "host:{host}\nif-none-match:*\nx-amz-content-sha256:{digest}\nx-amz-date:{date}\n"
            ),
            "host;if-none-match;x-amz-content-sha256;x-amz-date",
        )
    } else {
        (
            format!("host:{host}\nx-amz-content-sha256:{digest}\nx-amz-date:{date}\n"),
            "host;x-amz-content-sha256;x-amz-date",
        )
    };
    let canonical = format!("{method}\n{path}\n\n{headers}\n{signed}\n{digest}");
    let scope = format!("{}/{region}/s3/aws4_request", &date[..8]);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let day = hmac(format!("AWS4{secret}").as_bytes(), &date[..8]);
    let region = hmac(&day, region);
    let service = hmac(&region, "s3");
    let key = hmac(&service, "aws4_request");
    format!(
        "AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={}",
        hex(&hmac(&key, &to_sign))
    )
}
async fn bounded(mut response: reqwest::Response) -> Result<Vec<u8>, ApiError> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_OBJECT_BYTES as u64)
    {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        if bytes.len() + chunk.len() > MAX_OBJECT_BYTES {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[async_trait]
impl Objects for R2 {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let response = self.request(reqwest::Method::GET, key, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(failed());
        }
        Ok(Some(bounded(response).await?))
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let response = self.request(reqwest::Method::PUT, key, Some(bytes)).await?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            if self.get(key).await?.as_deref() == Some(bytes) {
                return Ok(());
            }
            return Err(ApiError::conflict("cloud_transcript_object_collision"));
        }
        if !response.status().is_success() {
            return Err(failed());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_matches_independent_s3_sdk_and_preserves_literal_object_paths() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/r2-signature.json")).unwrap();
        for row in fixture["vectors"].as_array().unwrap() {
            let value = |k: &str| row[k].as_str().unwrap();
            assert_eq!(
                sign(
                    fixture["access"].as_str().unwrap(),
                    fixture["secret"].as_str().unwrap(),
                    "auto",
                    value("method"),
                    value("path"),
                    value("host"),
                    value("date"),
                    value("digest"),
                    value("method") == "PUT"
                ),
                value("authorization")
            );
        }
        assert_eq!(
            encode("one/%2F !é/head.json", true),
            "one/%252F%20%21%C3%A9/head.json"
        );
        assert!(R2::new("http://localhost/", "bucket", "access", "secret").is_err());
        assert!(R2::new("https://host/path", "bucket", "access", "secret").is_err());
    }
    #[tokio::test]
    async fn immutable_transport_checks_existing_bytes_and_rejects_redirects() {
        use axum::{Router, body::Bytes, http::StatusCode, routing::put};
        use std::collections::HashMap;
        let objects = Arc::new(std::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let app = Router::new().route(
            "/{*key}",
            put({
                let objects = objects.clone();
                move |axum::extract::Path(key): axum::extract::Path<String>,
                      headers: axum::http::HeaderMap,
                      body: Bytes| {
                    let objects = objects.clone();
                    async move {
                        assert_eq!(headers["if-none-match"], "*");
                        assert!(
                            headers["authorization"]
                                .to_str()
                                .unwrap()
                                .contains("host;if-none-match;x-amz-content-sha256;x-amz-date")
                        );
                        let mut map = objects.lock().unwrap();
                        if map.contains_key(&key) {
                            return StatusCode::PRECONDITION_FAILED;
                        }
                        map.insert(key, body.to_vec());
                        StatusCode::OK
                    }
                }
            })
            .get({
                let objects = objects.clone();
                move |axum::extract::Path(key): axum::extract::Path<String>| {
                    let objects = objects.clone();
                    async move {
                        if key.ends_with("redirect") {
                            return (
                                StatusCode::TEMPORARY_REDIRECT,
                                [("location", "https://unexpected.invalid/")],
                                Vec::new(),
                            );
                        }
                        match objects.lock().unwrap().get(&key) {
                            Some(bytes) => (StatusCode::OK, [("location", "")], bytes.clone()),
                            None => (StatusCode::NOT_FOUND, [("location", "")], Vec::new()),
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut client = R2::new("https://fixture.invalid/", "bucket", "access", "secret").unwrap();
        client.endpoint = endpoint.parse().unwrap();
        client.put("a/%2F/é", b"ciphertext").await.unwrap();
        client.put("a/%2F/é", b"ciphertext").await.unwrap();
        assert_eq!(client.get("a/%2F/é").await.unwrap().unwrap(), b"ciphertext");
        assert_eq!(
            client.put("a/%2F/é", b"different").await.unwrap_err().code,
            "cloud_transcript_object_collision"
        );
        assert!(client.get("missing").await.unwrap().is_none());
        assert!(client.get("redirect").await.is_err());
        assert!(client.get("../outside").await.is_err());
        assert!(objects.lock().unwrap().contains_key("bucket/a/%2F/é"));
        server.abort();
    }
}
