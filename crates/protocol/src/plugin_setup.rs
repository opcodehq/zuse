//! Input rules shared by native and browser plugin setup.
use crate::plugins::{PluginApiKey, PluginRequest, PluginReturnTo};
use serde_json::Value;
use std::collections::BTreeMap;

/// Local servers are personal and bounded per account on each computer.
pub const MAX_LOCAL_SERVERS: usize = 50;

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

/// A local stdio server form. Environment values may be secrets, so no Debug.
#[derive(Default)]
pub struct LocalServerDraft {
    pub name: String,
    /// A command, or a pasted JSON object `{command,args,env}` or `{mcpServers:{..}}`.
    pub command: String,
    /// One literal argument per line; quote a line as a JSON string for an empty value.
    pub args: String,
    /// `NAME=value` per line.
    pub env: String,
}

impl LocalServerDraft {
    pub fn request(self, tenant: &str) -> Result<PluginRequest, &'static str> {
        if !tenant.starts_with("personal:") {
            return Err("Local servers can only be added to your personal workspace.");
        }
        let command = self.command.trim();
        let (command, args, env) = if command.starts_with('{') {
            if !self.args.trim().is_empty() || !self.env.trim().is_empty() {
                return Err(
                    "Paste the JSON configuration on its own, without separate arguments or environment.",
                );
            }
            pasted_config(command)?
        } else {
            let args = self
                .args
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(crate::custom_acp::decode_field)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "Quoted arguments must be valid JSON strings.")?;
            let mut env = BTreeMap::new();
            for line in self.env.lines().filter(|line| !line.trim().is_empty()) {
                let (key, value) = line
                    .split_once('=')
                    .ok_or("Enter environment variables as NAME=value, one per line.")?;
                let value = crate::custom_acp::decode_field(value)
                    .map_err(|_| "Quoted environment values must be valid JSON strings.")?;
                if env.insert(key.trim().to_owned(), value).is_some() {
                    return Err("Each environment variable can only be set once.");
                }
            }
            (command.to_owned(), args, env)
        };
        let name = self.name.trim().to_owned();
        validate_local_server(&name, &command, &args, &env)?;
        Ok(PluginRequest::AddLocalServer {
            tenant_id: tenant.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            name,
            command,
            args,
            env,
        })
    }
}

/// Accept the single-server stdio JSON commonly published with MCP servers.
fn pasted_config(
    text: &str,
) -> Result<(String, Vec<String>, BTreeMap<String, String>), &'static str> {
    const INVALID: &str = "Paste one stdio server as {\"command\", \"args\", \"env\"}.";
    let mut value: Value = serde_json::from_str(text).map_err(|_| INVALID)?;
    if let Some(servers) = value.get("mcpServers") {
        let servers = servers.as_object().ok_or(INVALID)?;
        if servers.len() != 1 {
            return Err("Paste exactly one MCP server configuration.");
        }
        value = servers.values().next().cloned().ok_or(INVALID)?;
    }
    let object = value.as_object().ok_or(INVALID)?;
    if object.get("type").is_some_and(|kind| kind != "stdio") {
        return Err("Only stdio servers can run locally. Use Remote for server URLs.");
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .ok_or(INVALID)?
        .trim()
        .to_owned();
    let args = match object.get("args") {
        None => vec![],
        Some(args) => args
            .as_array()
            .ok_or(INVALID)?
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned).ok_or(INVALID))
            .collect::<Result<_, _>>()?,
    };
    let env = match object.get("env") {
        None => BTreeMap::new(),
        Some(env) => env
            .as_object()
            .ok_or(INVALID)?
            .iter()
            .map(|(key, value)| Ok((key.clone(), value.as_str().ok_or(INVALID)?.to_owned())))
            .collect::<Result<_, &'static str>>()?,
    };
    Ok((command, args, env))
}

pub fn env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
}

/// Shared by the forms and by the engine before anything is launched or saved.
pub fn validate_local_server(
    name: &str,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> Result<(), &'static str> {
    if name.trim().is_empty() || name.chars().count() > 60 || name.contains(['\0', '\n', '\r']) {
        return Err("Enter a server name between 1 and 60 characters.");
    }
    if command.trim().is_empty() || command.len() > 8192 || command.contains(['\0', '\n', '\r']) {
        return Err("Enter a command or executable path.");
    }
    if args.len() > 128
        || args
            .iter()
            .any(|arg| arg.len() > 8192 || arg.contains('\0'))
        || args.iter().map(String::len).sum::<usize>() > 128 * 1024
    {
        return Err("Arguments exceed the launch limit or contain invalid characters.");
    }
    if env.keys().any(|key| !env_name(key)) {
        return Err("Environment variable names use letters, digits and underscores.");
    }
    if env.len() > 128
        || env
            .values()
            .any(|value| value.len() > 64 * 1024 || value.contains('\0'))
        || env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 128 * 1024
    {
        return Err("Environment values exceed the launch limit or contain invalid characters.");
    }
    Ok(())
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
    #[test]
    fn local_setup_parses_literal_lines_and_stays_personal() {
        let request = LocalServerDraft {
            name: " Files ".into(),
            command: " npx ".into(),
            args: "-y\n\n@scope/server --root ~/My Files\n\"\"\n".into(),
            env: "TOKEN=a=b\nEMPTY=\n".into(),
        }
        .request("personal:user")
        .unwrap();
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["action"], "addLocalServer");
        assert_eq!(value["name"], "Files");
        assert_eq!(value["command"], "npx");
        assert_eq!(
            value["args"],
            serde_json::json!(["-y", "@scope/server --root ~/My Files", ""])
        );
        assert_eq!(value["env"], serde_json::json!({"TOKEN":"a=b","EMPTY":""}));
        assert!(
            LocalServerDraft {
                name: "Files".into(),
                command: "npx".into(),
                ..Default::default()
            }
            .request("organization:team")
            .is_err()
        );
    }
    #[test]
    fn local_setup_accepts_one_pasted_stdio_server() {
        for pasted in [
            r#"{"command":"uvx","args":["server"],"env":{"KEY":"v"}}"#,
            r#"{"mcpServers":{"files":{"type":"stdio","command":"uvx","args":["server"],"env":{"KEY":"v"}}}}"#,
        ] {
            let PluginRequest::AddLocalServer {
                command, args, env, ..
            } = LocalServerDraft {
                name: "Files".into(),
                command: pasted.into(),
                ..Default::default()
            }
            .request("personal:user")
            .unwrap()
            else {
                panic!("expected a local server");
            };
            assert_eq!(
                (command.as_str(), args, env["KEY"].as_str()),
                ("uvx", vec!["server".to_owned()], "v")
            );
        }
        for pasted in [
            r#"{"type":"http","url":"https://tools.example"}"#,
            r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}"#,
            r#"{"command":"x","args":"not a list"}"#,
            r#"{"command":"x","env":{"KEY":1}}"#,
            r#"{"command":""}"#,
            "{not json",
        ] {
            assert!(
                LocalServerDraft {
                    name: "Files".into(),
                    command: pasted.into(),
                    ..Default::default()
                }
                .request("personal:user")
                .is_err(),
                "{pasted}"
            );
        }
    }
    #[test]
    fn local_setup_rejects_invalid_names_commands_and_environment_without_echoing_values() {
        for (name, command, env) in [
            ("", "npx", ""),
            (&*"x".repeat(61), "npx", ""),
            ("Files", "  ", ""),
            ("Files", "np\0x", ""),
            ("Files", "npx", "1BAD=secret-value"),
            ("Files", "npx", "BAD NAME=secret-value"),
            ("Files", "npx", "NO_EQUALS"),
            ("Files", "npx", "KEY=a\nKEY=b"),
            ("Files", "npx", "KEY=\"secret-value"),
        ] {
            let error = LocalServerDraft {
                name: name.into(),
                command: command.into(),
                env: env.into(),
                ..Default::default()
            }
            .request("personal:user")
            .err()
            .unwrap();
            assert!(!error.contains("secret-value"));
        }
    }
}
