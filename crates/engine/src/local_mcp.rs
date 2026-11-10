//! Find MCP servers already configured in this computer's agent apps so a
//! person can bring them to their Zuse account instead of re-entering them.
//! Reads user-level configs only; nothing here writes or uploads.
use std::{collections::BTreeMap, path::Path};
use zuse_protocol::plugin_setup::DetectedServer;

const MAX_SERVERS: usize = 200;
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_CONFIG_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn strings(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn env_map(value: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// `{"mcpServers": {name: {command,args,env} | {type,url,headers}}}`, shared by Claude and Cursor.
fn json_servers(source: &str, text: &str) -> Vec<DetectedServer> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return vec![];
    };
    let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) else {
        return vec![];
    };
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let url = entry.get("url").and_then(|v| v.as_str()).map(str::to_owned);
            let command = entry
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            (url.is_some() || command.is_some()).then(|| DetectedServer {
                source: source.into(),
                name: name.clone(),
                command: if url.is_some() { None } else { command },
                args: strings(entry.get("args")),
                env: env_map(entry.get("env")),
                headers: env_map(entry.get("headers")),
                transport: entry
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                url,
            })
        })
        .collect()
}

fn string_table(value: Option<&toml::Value>) -> BTreeMap<String, String> {
    value
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// Codex `[mcp_servers.<name>]` tables with `command`, `args`, `env` or `url`
/// with `http_headers`. Headers and bearer tokens named by environment variable
/// resolve from `var` now, as Codex would at launch; unset ones are skipped.
fn codex_servers(text: &str, var: impl Fn(&str) -> Option<String>) -> Vec<DetectedServer> {
    let Ok(value) = text.parse::<toml::Table>() else {
        return vec![];
    };
    let Some(servers) = value.get("mcp_servers").and_then(|v| v.as_table()) else {
        return vec![];
    };
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let entry = entry.as_table()?;
            let url = entry.get("url").and_then(|v| v.as_str()).map(str::to_owned);
            let command = entry
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let args = entry
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let env = string_table(entry.get("env"));
            let mut headers = string_table(entry.get("http_headers"));
            if url.is_some() {
                for (header, name) in string_table(entry.get("env_http_headers")) {
                    if let Some(value) = var(&name) {
                        headers.insert(header, value);
                    }
                }
                if let Some(token) = entry
                    .get("bearer_token_env_var")
                    .and_then(|v| v.as_str())
                    .and_then(&var)
                {
                    headers.insert("Authorization".into(), format!("Bearer {token}"));
                }
            }
            (url.is_some() || command.is_some()).then(|| DetectedServer {
                source: "Codex".into(),
                name: name.clone(),
                command: if url.is_some() { None } else { command },
                args,
                env,
                headers: if url.is_some() {
                    headers
                } else {
                    BTreeMap::new()
                },
                transport: None,
                url,
            })
        })
        .collect()
}

/// Every user-level MCP server under `home`, deduplicated across apps by
/// name and target so the same server configured twice appears once.
pub fn discover(home: &Path) -> Vec<DetectedServer> {
    let mut found = Vec::new();
    if let Some(text) = read(&home.join(".claude.json")) {
        found.extend(json_servers("Claude", &text));
    }
    if let Some(text) = read(&home.join(".codex/config.toml")) {
        found.extend(codex_servers(&text, |name| std::env::var(name).ok()));
    }
    if let Some(text) = read(&home.join(".cursor/mcp.json")) {
        found.extend(json_servers("Cursor", &text));
    }
    let mut seen = std::collections::BTreeSet::new();
    found.retain(|s| seen.insert((s.name.clone(), s.summary())));
    found.truncate(MAX_SERVERS);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_app_config_and_merges_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        std::fs::write(
            root.join(".claude.json"),
            r#"{"numStartups":3,"mcpServers":{"github":{"type":"stdio","command":"npx","args":["-y","@modelcontextprotocol/server-github"],"env":{"GITHUB_TOKEN":"secret"}},"docs":{"type":"http","url":"https://docs.example.com/mcp"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        std::fs::write(
            root.join(".codex/config.toml"),
            "model = \"x\"\n[mcp_servers.linear]\ncommand = \"npx\"\nargs = [\"-y\", \"linear-mcp\"]\n[mcp_servers.linear.env]\nLINEAR_KEY = \"k\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".cursor")).unwrap();
        // Same server as Claude's: shown once.
        std::fs::write(
            root.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"github":{"command":"npx","args":["-y","@modelcontextprotocol/server-github"]},"broken":{}}}"#,
        )
        .unwrap();
        let servers = discover(root);
        let names: Vec<_> = servers
            .iter()
            .map(|s| (s.source.as_str(), s.name.as_str()))
            .collect();
        // Each app's servers come back in name order.
        assert_eq!(
            names,
            [
                ("Claude", "docs"),
                ("Claude", "github"),
                ("Codex", "linear")
            ]
        );
        assert_eq!(servers[1].env["GITHUB_TOKEN"], "secret");
        assert_eq!(
            servers[0].url.as_deref(),
            Some("https://docs.example.com/mcp")
        );
        assert!(servers[0].command.is_none());
        assert_eq!(servers[2].summary(), "npx -y linear-mcp");
        assert_eq!(servers[2].env["LINEAR_KEY"], "k");
    }

    #[test]
    fn missing_or_malformed_configs_find_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(discover(home.path()).is_empty());
        std::fs::write(home.path().join(".claude.json"), "not json").unwrap();
        assert!(discover(home.path()).is_empty());
    }

    #[test]
    fn detected_servers_use_the_manual_add_rules() {
        let stdio = DetectedServer {
            source: "Claude".into(),
            name: "github".into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "server".into()],
            env: BTreeMap::from([("TOKEN".into(), "v".into())]),
            url: None,
            headers: BTreeMap::new(),
            transport: None,
        };
        assert!(matches!(
            stdio.request("personal:me", zuse_protocol::plugins::PluginReturnTo::Web),
            Ok(zuse_protocol::plugins::PluginRequest::AddLocalServer { .. })
        ));
        // Local servers stay personal, exactly like the manual form.
        assert!(
            stdio
                .request(
                    "organization:team",
                    zuse_protocol::plugins::PluginReturnTo::Web
                )
                .is_err()
        );
        // Plain HTTP can't go to the hosted service, so it becomes a local URL plugin.
        let insecure = DetectedServer {
            url: Some("http://plain.example/mcp".into()),
            command: None,
            env: BTreeMap::new(),
            ..stdio.clone()
        };
        assert!(matches!(
            insecure.request("personal:me", zuse_protocol::plugins::PluginReturnTo::Web),
            Ok(zuse_protocol::plugins::PluginRequest::AddLocalServer { url: Some(_), .. })
        ));
        assert!(
            insecure
                .request(
                    "organization:team",
                    zuse_protocol::plugins::PluginReturnTo::Web
                )
                .is_err()
        );
        let public = DetectedServer {
            url: Some("https://tools.example/mcp".into()),
            ..insecure
        };
        assert!(matches!(
            public.request(
                "organization:team",
                zuse_protocol::plugins::PluginReturnTo::Web
            ),
            Ok(zuse_protocol::plugins::PluginRequest::AddServer { .. })
        ));
    }

    #[test]
    fn url_servers_keep_their_headers_and_transport() {
        let claude = json_servers(
            "Claude",
            r#"{"mcpServers":{"dev":{"type":"sse","url":"http://localhost:3000/sse","headers":{"Authorization":"Bearer claude-secret","X-Team":"t"}},"files":{"command":"npx","headers":{"X":"ignored for stdio"}}}}"#,
        );
        assert_eq!(claude[0].transport.as_deref(), Some("sse"));
        assert_eq!(claude[0].headers["Authorization"], "Bearer claude-secret");
        assert_eq!(claude[0].headers["X-Team"], "t");
        assert!(!claude[0].summary().contains("secret"));
        assert!(!claude[0].hosted());
        assert!(claude[1].transport.is_none());

        let codex = codex_servers(
            "[mcp_servers.remote]\nurl = \"https://tools.example/mcp\"\nbearer_token_env_var = \"TOOLS_TOKEN\"\n[mcp_servers.remote.http_headers]\nX-Org = \"acme\"\n[mcp_servers.remote.env_http_headers]\nX-Key = \"TOOLS_KEY\"\nX-Unset = \"MISSING\"\n",
            |name| match name {
                "TOOLS_TOKEN" => Some("codex-secret".into()),
                "TOOLS_KEY" => Some("key".into()),
                _ => None,
            },
        );
        let headers = &codex[0].headers;
        assert_eq!(headers["Authorization"], "Bearer codex-secret");
        assert_eq!(headers["X-Org"], "acme");
        assert_eq!(headers["X-Key"], "key");
        assert!(!headers.contains_key("X-Unset"));
        // Several headers can't be expressed by the hosted form.
        assert!(!codex[0].hosted());
        assert!(matches!(
            codex[0].request("personal:me", zuse_protocol::plugins::PluginReturnTo::Web),
            Ok(zuse_protocol::plugins::PluginRequest::AddLocalServer { .. })
        ));
        assert_eq!(codex[0].summary(), "https://tools.example/mcp");
    }
}
