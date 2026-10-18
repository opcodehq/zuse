//! Input rules shared by native and browser plugin setup.
use crate::plugins::{PluginApiKey, PluginRequest, PluginReturnTo};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ServerAuth {
    #[default]
    Automatic,
    Bearer,
    Header,
}

// Credential drafts deliberately have no Debug or serialization implementation.
#[derive(Default)]
pub struct ServerDraft {
    pub name: String,
    pub url: String,
    pub auth: ServerAuth,
    pub header: String,
    pub secret: String,
}

pub fn api_key(header: &str, raw: &str) -> Result<PluginApiKey, &'static str> {
    let header = header.trim();
    if header.is_empty()
        || !header
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        return Err("Enter a valid HTTP header name.");
    }
    let value = raw.trim();
    if value.is_empty() || value.bytes().any(|b| b.is_ascii_control()) {
        return Err("Enter an API key without line breaks or control characters.");
    }
    let bearer = value
        .split_once(char::is_whitespace)
        .is_some_and(|(scheme, tail)| {
            scheme.eq_ignore_ascii_case("bearer") && !tail.trim().is_empty()
        });
    Ok(PluginApiKey {
        header: header.to_owned(),
        value: if header.eq_ignore_ascii_case("authorization") && !bearer {
            format!("Bearer {value}")
        } else {
            value.to_owned()
        },
    })
}

impl ServerDraft {
    pub fn request(
        self,
        tenant: &str,
        return_to: PluginReturnTo,
    ) -> Result<PluginRequest, &'static str> {
        if tenant.is_empty() {
            return Err("Choose a workspace before adding a server.");
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err("Enter a server name between 1 and 60 characters.");
        }
        let url = self.url.trim();
        let parsed = url::Url::parse(url).map_err(|_| "Enter an HTTPS server URL.")?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err("Enter an HTTPS server URL without embedded credentials or a fragment.");
        }
        let key = match self.auth {
            ServerAuth::Automatic => None,
            ServerAuth::Bearer => Some(api_key("Authorization", &self.secret)?),
            ServerAuth::Header => Some(api_key(&self.header, &self.secret)?),
        };
        Ok(PluginRequest::AddServer {
            tenant_id: tenant.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            url: url.into(),
            api_key: key,
            return_to: Some(return_to),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_preserves_tenant_authentication_and_secret_bytes() {
        for (auth, header, secret, expected) in [
            (ServerAuth::Automatic, "", "ignored", None),
            (
                ServerAuth::Bearer,
                "",
                " key-1 ",
                Some(("Authorization", "Bearer key-1")),
            ),
            (
                ServerAuth::Header,
                "X-API-Key",
                " key-2 ",
                Some(("X-API-Key", "key-2")),
            ),
            (
                ServerAuth::Header,
                "authorization",
                "bEaReR key-3",
                Some(("authorization", "bEaReR key-3")),
            ),
        ] {
            let request = ServerDraft {
                name: " Tool server ".into(),
                url: " https://tools.example/mcp?version=2 ".into(),
                auth,
                header: header.into(),
                secret: secret.into(),
            }
            .request("workspace-a", PluginReturnTo::Web)
            .unwrap();
            let value = serde_json::to_value(request).unwrap();
            assert_eq!(value["tenantId"], "workspace-a");
            assert_eq!(value["name"], "Tool server");
            assert_eq!(value["url"], "https://tools.example/mcp?version=2");
            assert_eq!(value["returnTo"]["kind"], "web");
            if let Some((h, v)) = expected {
                assert_eq!(value["apiKey"]["header"], h);
                assert_eq!(value["apiKey"]["value"], v);
            } else {
                assert!(value.get("apiKey").is_none());
            }
        }
    }
    #[test]
    fn setup_rejects_invalid_urls_and_header_injection_without_echoing_secrets() {
        for url in [
            "http://tools.example/mcp",
            "file:///etc/passwd",
            "https://user:secret@tools.example/mcp",
            "https://tools.example/#secret",
            "not a url",
        ] {
            assert!(
                ServerDraft {
                    name: "Tools".into(),
                    url: url.into(),
                    ..Default::default()
                }
                .request("tenant", PluginReturnTo::Web)
                .is_err()
            );
        }
        for (header, value) in [
            ("Bad Header", "secret"),
            ("X-Key\r\nOther", "secret"),
            ("X-Key", "secret\nother"),
            ("Authorization", ""),
        ] {
            let error = api_key(header, value).err().unwrap();
            assert!(!error.contains("secret"));
        }
        assert!(
            ServerDraft {
                name: "x".repeat(61),
                url: "https://tools.example".into(),
                ..Default::default()
            }
            .request("tenant", PluginReturnTo::Web)
            .is_err()
        );
    }
}
