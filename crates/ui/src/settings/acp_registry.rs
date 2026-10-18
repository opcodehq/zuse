//! The public ACP agent registry: the list behind the Add ACP agent dialog.
//!
//! The registry is fetched at runtime; offline, the dialog falls back to the
//! agents Zuse already ships as built-in providers. An entry distributed as an
//! npm or PyPI package becomes a custom ACP agent launched through `npx` or
//! `uvx`; a binary-only entry needs a manual install, so it prefills the
//! custom command form instead.
use serde::Deserialize;
use std::collections::BTreeMap;
use zuse_protocol::{HarnessId, custom_acp::CustomAcpAgent};

pub const REGISTRY_URL: &str =
    "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// Largest registry document accepted (the published one is ~60 KiB).
pub const MAX_REGISTRY_BYTES: usize = 2 * 1024 * 1024;
/// Largest icon accepted (icons are 16px SVGs).
pub const MAX_ICON_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize)]
struct Registry {
    agents: Vec<RegistryAgent>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub distribution: Distribution,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Distribution {
    #[serde(default)]
    pub npx: Option<Package>,
    #[serde(default)]
    pub uvx: Option<Package>,
    #[serde(default)]
    pub binary: BTreeMap<String, Binary>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Package {
    pub package: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Binary {
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// How the dialog offers one registry entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// Zuse ships this agent as a provider already.
    BuiltIn(HarnessId),
    /// One click adds a custom ACP agent that launches the package.
    Add(CustomAcpAgent),
    /// A binary to install by hand: the custom form, prefilled.
    Manual(CustomAcpAgent),
}

/// Registry entries that are Zuse's own built-in providers.
const BUILT_IN: [(&str, HarnessId, &str); 9] = [
    ("claude-acp", HarnessId::ClaudeCode, "Claude Code"),
    ("codex-acp", HarnessId::Codex, "Codex"),
    ("cursor", HarnessId::Cursor, "Cursor"),
    ("gemini", HarnessId::Gemini, "Gemini CLI"),
    ("devin", HarnessId::Devin, "Devin"),
    ("grok-build", HarnessId::Grok, "Grok"),
    ("opencode", HarnessId::Opencode, "OpenCode"),
    ("pi-acp", HarnessId::Pi, "Pi"),
    ("antigravity-acp", HarnessId::Antigravity, "Antigravity"),
];

pub fn built_in(registry_id: &str) -> Option<HarnessId> {
    BUILT_IN
        .iter()
        .find(|(id, ..)| *id == registry_id)
        .map(|(_, harness, _)| *harness)
}

/// The offline list: only the providers Zuse already ships.
pub fn fallback() -> Vec<RegistryAgent> {
    let mut agents: Vec<_> = BUILT_IN
        .iter()
        .map(|(id, _, name)| RegistryAgent {
            id: (*id).into(),
            name: (*name).into(),
            description: "Built in to Zuse".into(),
            ..Default::default()
        })
        .collect();
    agents.sort_by_key(|agent| agent.name.to_lowercase());
    agents
}

pub fn parse(bytes: &[u8]) -> Result<Vec<RegistryAgent>, String> {
    let registry: Registry = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut agents: Vec<_> = registry
        .agents
        .into_iter()
        .filter(|agent| !agent.id.is_empty() && !agent.name.trim().is_empty())
        .collect();
    agents.sort_by_key(|agent| agent.name.to_lowercase());
    agents.dedup_by(|a, b| a.id == b.id);
    Ok(agents)
}

/// The custom ACP identity a registry entry is added under.
pub fn custom_id(registry_id: &str) -> String {
    let mut slug = String::new();
    for ch in registry_id.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = slug.strip_suffix("-acp").unwrap_or(slug);
    let mut slug: String = slug.chars().take(96).collect();
    if slug.is_empty() {
        slug = "agent".into();
    }
    format!("acp-{slug}")
}

/// This machine's registry platform key (`darwin-aarch64`, `linux-x86_64`…).
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// How to offer `agent` on `platform`.
pub fn offer(agent: &RegistryAgent, platform: &str) -> Option<Offer> {
    if let Some(harness) = built_in(&agent.id) {
        return Some(Offer::BuiltIn(harness));
    }
    let id = HarnessId::try_from(custom_id(&agent.id)).ok()?;
    let custom = |command: &str, args: Vec<String>, env: &BTreeMap<String, String>| {
        let agent = CustomAcpAgent {
            id,
            name: agent.name.trim().to_owned(),
            command: command.to_owned(),
            args,
            env: env.clone(),
            model_ids: Vec::new(),
        };
        agent.validate().ok().map(|_| agent)
    };
    if let Some(package) = &agent.distribution.npx {
        let mut args = vec!["-y".to_owned(), package.package.clone()];
        args.extend(package.args.iter().cloned());
        return custom("npx", args, &package.env).map(Offer::Add);
    }
    if let Some(package) = &agent.distribution.uvx {
        let mut args = vec![package.package.clone()];
        args.extend(package.args.iter().cloned());
        return custom("uvx", args, &package.env).map(Offer::Add);
    }
    let binary = agent.distribution.binary.get(platform)?;
    // Installed by hand, the executable is on PATH under its own name.
    let name = binary
        .cmd
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&binary.cmd)
        .to_owned();
    custom(&name, binary.args.clone(), &binary.env).map(Offer::Manual)
}

/// Case-insensitive match on name, description and id.
pub fn matches(agent: &RegistryAgent, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || format!("{} {} {}", agent.name, agent.description, agent.id)
            .to_lowercase()
            .contains(&query)
}

/// Monochrome registry icons draw in `currentColor`; paint them in `color`
/// (`#rrggbb`) so they read on either theme.
pub fn tint_icon(svg: &str, color: &str) -> String {
    svg.replace("currentColor", color)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "version": "1.0.0",
        "agents": [
            {"id": "sample-acp", "name": "Sample", "version": "1.0.0", "description": "A sample agent",
             "icon": "https://cdn.example.test/sample.svg",
             "distribution": {"npx": {"package": "@example/sample@1.0.0", "args": ["--acp"], "env": {"SAMPLE_UPDATES": "0"}}}},
            {"id": "py-agent", "name": "Python agent", "distribution": {"uvx": {"package": "py-agent==0.1"}}},
            {"id": "native", "name": "Native", "distribution": {"binary": {
                "linux-x86_64": {"archive": "https://example.test/a.tgz", "cmd": "./native-acp", "args": ["serve"]}}}},
            {"id": "codex-acp", "name": "Codex", "distribution": {"npx": {"package": "@example/codex"}}},
            {"id": "", "name": "Nameless"}
        ],
        "extensions": []
    }"#;

    #[test]
    fn parses_sorts_and_drops_invalid_entries() {
        let agents = parse(SAMPLE.as_bytes()).unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Codex", "Native", "Python agent", "Sample"]);
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn packages_become_launchable_custom_agents() {
        let agents = parse(SAMPLE.as_bytes()).unwrap();
        let sample = agents.iter().find(|a| a.id == "sample-acp").unwrap();
        let Some(Offer::Add(agent)) = offer(sample, "linux-x86_64") else {
            panic!("npx entries add in one click");
        };
        assert_eq!(String::from(agent.id), "acp-sample");
        assert_eq!(agent.command, "npx");
        assert_eq!(agent.args, ["-y", "@example/sample@1.0.0", "--acp"]);
        assert_eq!(agent.env["SAMPLE_UPDATES"], "0");
        let python = agents.iter().find(|a| a.id == "py-agent").unwrap();
        let Some(Offer::Add(agent)) = offer(python, "linux-x86_64") else {
            panic!("uvx entries add in one click");
        };
        assert_eq!(
            (agent.command.as_str(), agent.args.as_slice()),
            ("uvx", &["py-agent==0.1".to_owned()][..])
        );
    }

    #[test]
    fn binaries_prefill_and_built_ins_are_not_duplicated() {
        let agents = parse(SAMPLE.as_bytes()).unwrap();
        let native = agents.iter().find(|a| a.id == "native").unwrap();
        let Some(Offer::Manual(agent)) = offer(native, "linux-x86_64") else {
            panic!("binary entries prefill the custom form");
        };
        assert_eq!(
            (agent.command.as_str(), agent.args.as_slice()),
            ("native-acp", &["serve".to_owned()][..])
        );
        assert_eq!(offer(native, "windows-aarch64"), None);
        let codex = agents.iter().find(|a| a.id == "codex-acp").unwrap();
        assert_eq!(
            offer(codex, "linux-x86_64"),
            Some(Offer::BuiltIn(HarnessId::Codex))
        );
        assert!(fallback().iter().all(|agent| built_in(&agent.id).is_some()));
    }

    #[test]
    fn custom_ids_are_valid_acp_identities() {
        for (registry, expected) in [
            ("amp-acp", "acp-amp"),
            ("Fast_Agent", "acp-fast-agent"),
            ("--", "acp-agent"),
            ("x.y.z", "acp-x-y-z"),
        ] {
            assert_eq!(custom_id(registry), expected);
            assert!(HarnessId::try_from(custom_id(registry)).is_ok());
        }
        assert!(matches(&fallback()[0], ""));
        assert_eq!(
            tint_icon(r#"<path fill="currentColor"/>"#, "#ffffff"),
            r##"<path fill="#ffffff"/>"##
        );
    }
}
