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

/// An MCP server found in an agent app's config on this computer. Stdio
/// servers carry their env, which may hold secrets: never log this value.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedServer {
    /// Which app's config it came from ("Claude", "Codex", "Cursor").
    pub source: String,
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl DetectedServer {
    /// One-line summary safe to show: the command or URL, never env values.
    pub fn summary(&self) -> String {
        match (&self.url, &self.command) {
            (Some(url), _) => url.clone(),
            (None, Some(command)) => std::iter::once(command.as_str())
                .chain(self.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            (None, None) => String::new(),
        }
    }
    /// The same request the manual Add MCP server form would send, so every
    /// existing validation applies to imports too.
    pub fn request(
        &self,
        tenant: &str,
        return_to: PluginReturnTo,
    ) -> Result<PluginRequest, &'static str> {
        if let Some(url) = &self.url {
            return ServerDraft {
                name: self.name.clone(),
                url: url.clone(),
                ..Default::default()
            }
            .request(tenant, return_to);
        }
        let command = self.command.clone().ok_or(LOCAL_CONFIG_INVALID)?;
        LocalServerDraft {
            name: self.name.clone(),
            config: serde_json::json!({"command": command, "args": self.args, "env": self.env}).to_string(),
        }
        .request(tenant)
    }
}

/// The Local form starts from this editable stdio configuration.
pub const LOCAL_CONFIG_TEMPLATE: &str = r#"{"command":"","args":[]}"#;
pub const LOCAL_CONFIG_INVALID: &str = "Enter a valid stdio MCP configuration with one command, an args array, and optional env values.";

/// A local stdio server form. Environment values may be secrets, so no Debug.
#[derive(Default)]
pub struct LocalServerDraft {
    pub name: String,
    /// One stdio server as `{command,args,env}` or `{"mcpServers":{<one entry>}}`.
    pub config: String,
}

impl LocalServerDraft {
    pub fn request(self, tenant: &str) -> Result<PluginRequest, &'static str> {
        if !tenant.starts_with("personal:") {
            return Err("Local servers can only be added to your personal workspace.");
        }
        let name = self.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(NAME_INVALID);
        }
        let (command, args, env) = parse_local_config(&self.config).ok_or(LOCAL_CONFIG_INVALID)?;
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

const NAME_INVALID: &str = "Enter a server name between 1 and 60 characters.";

/// Accept the single-server stdio JSON commonly published with MCP servers.
fn parse_local_config(text: &str) -> Option<(String, Vec<String>, BTreeMap<String, String>)> {
    let mut value: Value = serde_json::from_str(text).ok()?;
    if let Some(servers) = value.get("mcpServers") {
        let servers = servers.as_object()?;
        if servers.len() != 1 {
            return None;
        }
        value = servers.values().next()?.clone();
    }
    let object = value.as_object()?;
    if object.get("type").is_some_and(|kind| kind != "stdio") {
        return None;
    }
    let command = object.get("command")?.as_str()?.trim().to_owned();
    if command.is_empty() {
        return None;
    }
    let args = match object.get("args") {
        None => vec![],
        Some(args) => args
            .as_array()?
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
    };
    let env = match object.get("env") {
        None => BTreeMap::new(),
        Some(env) => env
            .as_object()?
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect::<Option<_>>()?,
    };
    Some((command, args, env))
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
        return Err(NAME_INVALID);
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
    fn local(name: &str, config: &str, tenant: &str) -> Result<PluginRequest, &'static str> {
        LocalServerDraft {
            name: name.into(),
            config: config.into(),
        }
        .request(tenant)
    }
    #[test]
    fn local_setup_accepts_a_bare_server_or_one_wrapped_server_and_stays_personal() {
        for config in [
            r#"{"command":" uvx ","args":["server",""],"env":{"KEY":"a=b"}}"#,
            r#"{"type":"stdio","command":"uvx","args":["server",""],"env":{"KEY":"a=b"}}"#,
            r#"{"mcpServers":{"files":{"type":"stdio","command":"uvx","args":["server",""],"env":{"KEY":"a=b"}}}}"#,
        ] {
            let value =
                serde_json::to_value(local(" Files ", config, "personal:user").unwrap()).unwrap();
            assert_eq!(value["action"], "addLocalServer");
            assert_eq!(value["name"], "Files");
            assert_eq!(value["command"], "uvx");
            assert_eq!(value["args"], serde_json::json!(["server", ""]));
            assert_eq!(value["env"], serde_json::json!({"KEY":"a=b"}));
        }
        let PluginRequest::AddLocalServer { args, env, .. } =
            local("Files", r#"{"command":"node"}"#, "personal:user").unwrap()
        else {
            panic!("expected a local server");
        };
        assert!(args.is_empty() && env.is_empty());
        assert!(local("Files", r#"{"command":"node"}"#, "organization:team").is_err());
    }
    #[test]
    fn local_setup_rejects_invalid_configurations() {
        for config in [
            "null",
            "{}",
            "{not json",
            LOCAL_CONFIG_TEMPLATE,
            r#"{"command":""}"#,
            r#"{"command":"   "}"#,
            r#"{"command":1}"#,
            r#"{"command":"node","type":"http"}"#,
            r#"{"type":"http","url":"https://tools.example"}"#,
            r#"{"command":"node","args":"not a list"}"#,
            r#"{"command":"node","args":[1]}"#,
            r#"{"command":"node","env":{"TOKEN":1}}"#,
            r#"{"command":"node","env":["TOKEN=x"]}"#,
            r#"{"mcpServers":{}}"#,
            r#"{"mcpServers":{"a":{"command":"x"},"b":{"command":"y"}}}"#,
            r#"{"mcpServers":{"a":{}}}"#,
        ] {
            assert_eq!(
                local("Files", config, "personal:user").err(),
                Some(LOCAL_CONFIG_INVALID),
                "{config}"
            );
        }
    }
    #[test]
    fn local_setup_rejects_invalid_names_commands_and_environment_without_echoing_values() {
        for (name, config) in [
            ("", r#"{"command":"npx"}"#),
            ("  ", r#"{"command":"npx"}"#),
            (&*"x".repeat(61), r#"{"command":"npx"}"#),
            ("Files", r#"{"command":"np\u0000x"}"#),
            (
                "Files",
                r#"{"command":"npx","env":{"1BAD":"secret-value"}}"#,
            ),
            (
                "Files",
                r#"{"command":"npx","env":{"BAD NAME":"secret-value"}}"#,
            ),
            (
                "Files",
                r#"{"command":"npx","env":{"KEY":"secret-value\u0000"}}"#,
            ),
        ] {
            let error = local(name, config, "personal:user").err().unwrap();
            assert!(!error.contains("secret-value"), "{config}");
        }
        assert_eq!(
            local("", r#"{"command":"npx"}"#, "personal:user").err(),
            Some(NAME_INVALID)
        );
    }
}
