//! Bounded provider transport. Never retries mutations or follows credential redirects.
use super::ProviderError;
use reqwest::{Method, Url};
use serde_json::Value;
use std::time::Duration;
use zeroize::Zeroizing;

pub(super) struct Rest {
    pub http: reqwest::Client,
    base: Url,
    secret: Zeroizing<String>,
    header: &'static str,
    bearer: bool,
}
impl Rest {
    pub fn trusted_request(
        &self,
        method: Method,
        url: Url,
        allowed_origins: &[&str],
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let secure = url.scheme() == "https";
        #[cfg(test)]
        let secure = secure || (url.scheme() == "http" && url.host_str() == Some("127.0.0.1"));
        if !secure
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !allowed_origins
                .iter()
                .any(|origin| *origin == url.origin().ascii_serialization())
        {
            return Err(ProviderError::Rejected);
        }
        let auth = Zeroizing::new(if self.bearer {
            format!("Bearer {}", self.secret.as_str())
        } else {
            self.secret.to_string()
        });
        Ok(self
            .http
            .request(method, url)
            .header(self.header, auth.as_str()))
    }
    pub fn new(
        base: &str,
        secret: String,
        header: &'static str,
        bearer: bool,
    ) -> Result<Self, ProviderError> {
        let url = Url::parse(base).map_err(|_| ProviderError::Rejected)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || secret.is_empty()
            || secret.len() > 8192
            || secret.contains(['\r', '\n', '\0'])
        {
            return Err(ProviderError::Rejected);
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(45))
                .build()
                .map_err(|_| ProviderError::Unavailable)?,
            base: url,
            secret: Zeroizing::new(secret),
            header,
            bearer,
        })
    }
    pub fn route(&self, segments: &[&str]) -> Result<Url, ProviderError> {
        if segments.iter().any(|s| {
            s.is_empty()
                || s.len() > 512
                || *s == "."
                || *s == ".."
                || s.chars().any(char::is_control)
        }) {
            return Err(ProviderError::Rejected);
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| ProviderError::Rejected)?
            .pop_if_empty()
            .extend(segments);
        Ok(url)
    }
    pub async fn request(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
        idempotency: Option<&str>,
        delete_confirmation: Option<&str>,
    ) -> Result<Value, ProviderError> {
        // Only the configured control-plane origin receives infrastructure keys.
        if url.origin() != self.base.origin() {
            return Err(ProviderError::Rejected);
        }
        let mutation = method != Method::GET;
        let auth = Zeroizing::new(if self.bearer {
            format!("Bearer {}", self.secret.as_str())
        } else {
            self.secret.to_string()
        });
        let mut request = self
            .http
            .request(method, url)
            .header(self.header, auth.as_str());
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(key) = idempotency {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(id) = delete_confirmation {
            request = request.header("X-Ascii-Confirm-Delete", id);
        }
        let response = request.send().await.map_err(|_| {
            if mutation {
                ProviderError::OutcomeUnknown
            } else {
                ProviderError::Unavailable
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status.as_u16() {
                404 => ProviderError::NotFound,
                409 => ProviderError::Conflict,
                429 => ProviderError::RateLimited,
                400..=499 => ProviderError::Rejected,
                _ if mutation => ProviderError::OutcomeUnknown,
                _ => ProviderError::Unavailable,
            });
        }
        if status.as_u16() == 204 {
            return Ok(Value::Null);
        }
        let bytes = read(response, 8 * 1024 * 1024).await.map_err(|error| {
            if mutation {
                ProviderError::OutcomeUnknown
            } else {
                error
            }
        })?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            if mutation {
                ProviderError::OutcomeUnknown
            } else {
                ProviderError::InvalidResponse
            }
        })
    }
    #[cfg(test)]
    pub fn test_endpoint(&mut self, base: String) {
        self.base = Url::parse(&base).unwrap();
    }
}
pub(super) async fn read(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderError::Unavailable)?
    {
        if bytes.len() + chunk.len() > limit {
            return Err(ProviderError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub(super) fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, ProviderError> {
    value[name]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
        .ok_or(ProviderError::InvalidResponse)
}
