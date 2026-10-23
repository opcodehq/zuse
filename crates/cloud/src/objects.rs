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
    fn supports_purge(&self) -> bool {
        false
    }
    /// Delete one bounded page. True only after an empty, scoped listing proves completion.
    async fn purge_prefix(&self, _prefix: &str) -> Result<bool, ApiError> {
        Err(ApiError::unavailable("cloud_object_purge_unsupported"))
    }
    async fn get_archive(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        self.get(key).await
    }
    async fn put_archive(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        self.put(key, bytes).await
    }
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
        limit: usize,
    ) -> Result<reqwest::Response, ApiError> {
        if key.is_empty()
            || key.len() > 2048
            || key.split('/').any(|s| s == "." || s == "..")
            || body.is_some_and(|b| b.len() > limit)
        {
            return Err(failed());
        }
        // Encode literal percent signs again: reference object names already contain encodeURIComponent segments.
        let path = format!("/{}/{}", self.bucket, encode(key, true));
        self.request_path(method, &path, "", body, limit).await
    }
    async fn request_path(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &str,
        body: Option<&[u8]>,
        limit: usize,
    ) -> Result<reqwest::Response, ApiError> {
        if body.is_some_and(|b| b.len() > limit) {
            return Err(failed());
        }
        let mut url = self.endpoint.clone();
        url.set_path(path);
        if !query.is_empty() {
            url.set_query(Some(query));
        }
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
        let authorization = sign_query(
            &self.access,
            &self.secret,
            "auto",
            method.as_str(),
            path,
            query,
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
pub(crate) fn encode(value: &str, slash: bool) -> String {
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
#[cfg(test)]
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
    sign_query(
        access, secret, region, method, path, "", host, date, digest, immutable,
    )
}
fn sign_query(
    access: &str,
    secret: &str,
    region: &str,
    method: &str,
    path: &str,
    query: &str,
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
    let canonical = format!("{method}\n{path}\n{query}\n{headers}\n{signed}\n{digest}");
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
async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ApiError> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        if bytes.len() + chunk.len() > limit {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[async_trait]
impl Objects for R2 {
    fn supports_purge(&self) -> bool {
        true
    }
    async fn purge_prefix(&self, prefix: &str) -> Result<bool, ApiError> {
        use futures::{StreamExt, TryStreamExt};
        let segments: Vec<_> = prefix.split('/').collect();
        if segments.len() != 3
            || segments[0] != "workspaces"
            || segments[1].is_empty()
            || segments[2] != ""
            || matches!(segments[1], "." | "..")
        {
            return Err(ApiError::unavailable("cloud_object_purge_scope_invalid"));
        }
        let query = format!(
            "encoding-type=url&list-type=2&max-keys=100&prefix={}",
            encode(prefix, false)
        );
        let response = self
            .request_path(
                reqwest::Method::GET,
                &format!("/{}/", self.bucket),
                &query,
                None,
                1024 * 1024,
            )
            .await?;
        if !response.status().is_success() {
            return Err(failed());
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Page {
            is_truncated: bool,
            name: String,
            prefix: String,
            encoding_type: String,
            #[serde(default, rename = "Contents")]
            contents: Vec<Entry>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            #[serde(rename = "Key")]
            key: String,
        }
        let bytes = bounded(response, 1024 * 1024).await?;
        let page: Page = quick_xml::de::from_reader(bytes.as_slice()).map_err(|_| failed())?;
        if page.name != self.bucket
            || page.encoding_type != "url"
            || percent_encoding::percent_decode_str(&page.prefix)
                .decode_utf8()
                .map_err(|_| failed())?
                != prefix
        {
            return Err(failed());
        }
        if page.contents.len() > 100 || page.contents.is_empty() && page.is_truncated {
            return Err(failed());
        }
        if page.contents.is_empty() {
            return Ok(true);
        }
        let keys: Vec<String> = page
            .contents
            .into_iter()
            .map(|e| {
                let key = percent_encoding::percent_decode_str(&e.key)
                    .decode_utf8()
                    .map_err(|_| failed())?
                    .into_owned();
                if !key.starts_with(prefix) {
                    return Err(ApiError::unavailable("cloud_object_purge_scope_invalid"));
                }
                Ok(key)
            })
            .collect::<Result<_, ApiError>>()?;
        futures::stream::iter(keys)
            .map(|key| async move {
                let response = self.request(reqwest::Method::DELETE, &key, None, 0).await?;
                if !response.status().is_success()
                    && response.status() != reqwest::StatusCode::NOT_FOUND
                {
                    return Err(failed());
                }
                Ok(())
            })
            .buffer_unordered(8)
            .try_collect::<Vec<_>>()
            .await?;
        Ok(false)
    }
    async fn get_archive(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let response = self
            .request(reqwest::Method::GET, key, None, 6 * 1024 * 1024)
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(failed());
        }
        Ok(Some(bounded(response, 6 * 1024 * 1024).await?))
    }
    async fn put_archive(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let response = self
            .request(reqwest::Method::PUT, key, Some(bytes), 6 * 1024 * 1024)
            .await?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            if self.get_archive(key).await?.as_deref() == Some(bytes) {
                return Ok(());
            }
            return Err(ApiError::conflict("cloud_archive_object_collision"));
        }
        if !response.status().is_success() {
            return Err(failed());
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, ApiError> {
        let response = self
            .request(reqwest::Method::GET, key, None, MAX_OBJECT_BYTES)
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(failed());
        }
        Ok(Some(bounded(response, MAX_OBJECT_BYTES).await?))
    }
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), ApiError> {
        let response = self
            .request(reqwest::Method::PUT, key, Some(bytes), MAX_OBJECT_BYTES)
            .await?;
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
        let app = app.layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024));
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
        let large = vec![b'x'; 5 * 1024 * 1024];
        assert!(client.put("transcript-large", &large).await.is_err());
        client.put_archive("archive-large", &large).await.unwrap();
        client.put_archive("archive-large", &large).await.unwrap();
        assert_eq!(
            client.get_archive("archive-large").await.unwrap().unwrap(),
            large
        );
        assert!(client.get("archive-large").await.is_err());
        assert!(
            client
                .put_archive("archive-large", b"conflict")
                .await
                .is_err()
        );
        assert!(
            client
                .put_archive("too-large", &vec![0; 6 * 1024 * 1024 + 1])
                .await
                .is_err()
        );
        server.abort();
    }
    #[test]
    fn list_and_delete_signatures_match_the_independent_s3_sdk() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/r2-delete-signatures.json"))
                .unwrap();
        for row in fixture["vectors"].as_array().unwrap() {
            let value = |key: &str| row[key].as_str().unwrap();
            assert_eq!(
                sign_query(
                    "ACCESS",
                    "SECRET",
                    "auto",
                    value("method"),
                    value("path"),
                    value("query"),
                    value("host"),
                    value("date"),
                    value("digest"),
                    false
                ),
                value("authorization")
            );
        }
    }
    #[tokio::test]
    async fn scoped_purge_retries_pages_and_refuses_unsafe_or_malformed_listings() {
        use axum::{
            Router,
            extract::{Path, Query},
            http::StatusCode,
            routing::get,
        };
        use std::{
            collections::HashMap,
            sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let map = Arc::new(std::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let mode = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(AtomicBool::new(true));
        let prefix = "workspaces/one%2Ftwo/";
        for i in 0..102 {
            map.lock()
                .unwrap()
                .insert(format!("{prefix}{i:03} % é"), vec![1]);
        }
        map.lock()
            .unwrap()
            .insert("workspaces/one%2Ftwo-lookalike/keep".into(), vec![2]);
        let app=Router::new().route("/bucket/",get({let map=map.clone();let mode=mode.clone();move|Query(query):Query<HashMap<String,String>>|{let map=map.clone();let mode=mode.clone();async move{
            assert_eq!(query["list-type"],"2");assert_eq!(query["encoding-type"],"url");assert_eq!(query["max-keys"],"100");
            let prefix=&query["prefix"];
            if mode.load(Ordering::SeqCst)==2{return (StatusCode::OK,"<Error><Code>AccessDenied</Code></Error>".to_owned())}
            let mut keys:Vec<_>=map.lock().unwrap().keys().filter(|k|k.starts_with(prefix)).cloned().collect();keys.sort();let more=keys.len()>100;keys.truncate(100);
            if mode.load(Ordering::SeqCst)==1{keys.push("workspaces/another-account/keep".into());}
            let entries=keys.iter().map(|k|format!("<Contents><Key>{}</Key></Contents>",encode(k,false))).collect::<String>();
            (StatusCode::OK,format!("<ListBucketResult><Name>bucket</Name><Prefix>{}</Prefix><EncodingType>url</EncodingType><IsTruncated>{more}</IsTruncated>{entries}</ListBucketResult>",encode(prefix,false)))
        }}})).route("/{*key}",axum::routing::delete({let map=map.clone();let fail=fail.clone();move|Path(key):Path<String>|{let map=map.clone();let fail=fail.clone();async move{
            let key=key.strip_prefix("bucket/").unwrap();
            if fail.swap(false,Ordering::SeqCst){return StatusCode::SERVICE_UNAVAILABLE}
            map.lock().unwrap().remove(key);StatusCode::NO_CONTENT
        }}}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut client = R2::new("https://fixture.invalid/", "bucket", "access", "secret").unwrap();
        client.endpoint = endpoint.parse().unwrap();
        assert!(client.purge_prefix("workspaces/").await.is_err());
        mode.store(2, Ordering::SeqCst);
        assert!(client.purge_prefix(prefix).await.is_err());
        assert_eq!(map.lock().unwrap().len(), 103);
        mode.store(1, Ordering::SeqCst);
        assert!(client.purge_prefix(prefix).await.is_err());
        assert_eq!(map.lock().unwrap().len(), 103);
        mode.store(0, Ordering::SeqCst);
        assert!(client.purge_prefix(prefix).await.is_err());
        for _ in 0..5 {
            if client.purge_prefix(prefix).await.unwrap() {
                break;
            }
        }
        assert!(client.purge_prefix(prefix).await.unwrap());
        assert_eq!(map.lock().unwrap().len(), 1);
        assert!(
            map.lock()
                .unwrap()
                .contains_key("workspaces/one%2Ftwo-lookalike/keep")
        );
        server.abort();
    }
}
