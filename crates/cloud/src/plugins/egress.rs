//! The single outbound policy for MCP endpoints, OAuth metadata, registration,
//! token calls and authorization URLs: public HTTPS hostnames on the default
//! port, never following redirects.
//!
//! Unlike the Workers runtime, this process can reach private networks, so the
//! hostname policy is backed by a resolver that drops every non-public address.
use super::PluginFailure;
use futures::StreamExt;
use reqwest::{
    Client, StatusCode, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    header::{HeaderMap, HeaderName, HeaderValue},
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

const BLOCKED_SUFFIXES: [&str; 3] = [".localhost", ".local", ".internal"];
pub const PROBE_PROTOCOL: &str = "2025-06-18";

/// `Loopback` exists only so tests can serve fixtures from 127.0.0.1 over HTTP.
/// Configuration from the environment always uses `Public`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Public,
    #[doc(hidden)]
    Loopback,
}

pub fn allowed(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| public_url(&url))
}

fn public_url(url: &Url) -> bool {
    // `domain()` is None for IP literals, including canonicalized shorthands.
    let Some(host) = url.domain() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && host.contains('.')
        && !host.ends_with('.')
        && host != "localhost"
        && !BLOCKED_SUFFIXES.iter().any(|suffix| host.ends_with(suffix))
}

impl Policy {
    pub fn check(self, url: &Url) -> Result<(), PluginFailure> {
        let ok = match self {
            Self::Public => public_url(url),
            Self::Loopback => {
                public_url(url)
                    || (url.scheme() == "http"
                        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
                        && url.username().is_empty())
            }
        };
        if ok {
            Ok(())
        } else {
            Err(PluginFailure::UrlRejected)
        }
    }
    pub fn parse(self, value: &str) -> Result<Url, PluginFailure> {
        let url = Url::parse(value).map_err(|_| PluginFailure::UrlRejected)?;
        self.check(&url)?;
        Ok(url)
    }

    pub fn client(self) -> anyhow::Result<Client> {
        let builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("Zuse");
        Ok(match self {
            Self::Public => builder.dns_resolver(Arc::new(PublicResolver)),
            Self::Loopback => builder,
        }
        .build()?)
    }
}

struct PublicResolver;
impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| public_ip(address.ip()))
                .collect();
            if addresses.is_empty() {
                return Err("plugin host does not resolve to a public address".into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_v4(v4);
            }
            let s = v6.segments();
            // Only global unicast. This also excludes IPv4-compatible,
            // site-local and translation prefixes outside 2000::/3.
            (s[0] & 0xe000) == 0x2000
                && !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (s[0] & 0xfe00) == 0xfc00
                    || (s[0] & 0xffc0) == 0xfe80
                    || (s[0] == 0x2001 && s[1] == 0x0db8)
                    || (s[0] == 0x2001 && s[1] < 0x0200)
                    || (s[0] & 0xfff0) == 0x3ff0
                    || (s[0] == 0x0064 && s[1] == 0xff9b)
                    || (s[0] == 0x2002))
        }
    }
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19)))
}

/// Reads at most `limit` bytes; larger bodies are an invalid response.
pub async fn bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, PluginFailure> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(PluginFailure::OperationFailed);
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| PluginFailure::Unreachable)?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(PluginFailure::OperationFailed);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub struct Probe {
    pub status: StatusCode,
    /// RFC 9728 `resource_metadata` from a `WWW-Authenticate` challenge.
    pub resource_metadata: Option<String>,
}

/// One anonymous (or keyed) MCP `initialize`; only status and headers are read.
pub async fn probe(
    http: &Client,
    policy: Policy,
    endpoint: &str,
    headers: HeaderMap,
) -> Result<Probe, PluginFailure> {
    let url = policy.parse(endpoint)?;
    let response = http
        .post(url.clone())
        .headers(headers.clone())
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .timeout(Duration::from_secs(10))
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROBE_PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "zuse-auth-probe", "version": "1.0.0"}
            }
        }))
        .send()
        .await
        .map_err(|_| PluginFailure::Unreachable)?;
    let response = if matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ) {
        drop(response);
        http.get(url.clone())
            .headers(headers.clone())
            .header("accept", "text/event-stream")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| PluginFailure::Unreachable)?
    } else {
        response
    };
    let status = response.status();
    let resource_metadata = response
        .headers()
        .get_all("www-authenticate")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| challenge_param(value, "resource_metadata"));
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok());
    drop(response);
    if status.is_success() {
        if let Some(session) = session {
            let mut release = headers;
            release.insert(HeaderName::from_static("mcp-session-id"), session);
            release.insert(
                HeaderName::from_static("mcp-protocol-version"),
                HeaderValue::from_static(PROBE_PROTOCOL),
            );
            let _ = http
                .delete(url)
                .headers(release)
                .timeout(Duration::from_secs(1))
                .send()
                .await;
        }
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(PluginFailure::Unreachable);
    }
    Ok(Probe {
        status,
        resource_metadata,
    })
}

/// Extracts one auth-param from a `WWW-Authenticate` value (quoted or token form).
pub fn challenge_param(header: &str, name: &str) -> Option<String> {
    let mut rest = header;
    while let Some(index) = rest.find(name) {
        let before = rest[..index].chars().last();
        let after = &rest[index + name.len()..];
        rest = after;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let Some(value) = after.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return if let Some(quoted) = value.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = quoted.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => out.push(chars.next()?),
                    '"' => return Some(out),
                    c => out.push(c),
                }
            }
            None
        } else {
            Some(
                value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .next()?
                    .to_owned(),
            )
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_accepts_only_public_https_hostnames() {
        for ok in [
            "https://mcp.linear.app/mcp",
            "https://example.com",
            "https://example.com:443/x?y=1",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "http://mcp.linear.app/mcp",
            "https://user:pw@example.com",
            "https://example.com:8443",
            "https://localhost",
            "https://foo.localhost",
            "https://printer.local",
            "https://svc.internal",
            "https://intranet",
            "https://example.com.",
            "https://127.0.0.1",
            "https://0x7f.1",
            "https://[::1]",
            "https://10.0.0.1",
            "ftp://example.com",
            "not a url",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
        assert!(Policy::Loopback.parse("http://127.0.0.1:9/mcp").is_ok());
        assert!(Policy::Public.parse("http://127.0.0.1:9/mcp").is_err());
    }

    #[test]
    fn resolver_filter_drops_private_addresses() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "::127.0.0.1",
            "fec0::1",
            "2001::1",
            "2002:7f00:1::1",
            "3fff::1",
        ] {
            assert!(!public_ip(bad.parse().unwrap()), "{bad}");
        }
        for ok in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ok.parse().unwrap()), "{ok}");
        }
    }

    #[test]
    fn challenge_parameters_parse_quoted_and_token_forms() {
        assert_eq!(
            challenge_param(
                r#"Bearer realm="x", resource_metadata="https://a.example/.well-known/oauth-protected-resource""#,
                "resource_metadata"
            )
            .as_deref(),
            Some("https://a.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            challenge_param(
                "Bearer resource_metadata=https://a.example/m, scope=x",
                "resource_metadata"
            )
            .as_deref(),
            Some("https://a.example/m")
        );
        assert_eq!(
            challenge_param(r#"Bearer xresource_metadata="no""#, "resource_metadata"),
            None
        );
    }
}
