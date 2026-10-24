//! Stable device-owned ACP identities and executable definitions.
pub mod command;

use crate::HarnessId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

/// Inline storage keeps harness IDs copyable without interning untrusted wire IDs.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CustomAcpId {
    bytes: [u8; 100],
    len: u8,
}
impl CustomAcpId {
    pub fn parse(value: &str) -> Result<Self, String> {
        let slug = value
            .strip_prefix("acp-")
            .ok_or("Custom ACP IDs must start with acp-")?;
        if slug.is_empty()
            || slug.len() > 96
            || !slug.as_bytes()[0].is_ascii_alphanumeric()
            || !slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("Custom ACP IDs need 1–96 lowercase letters, digits or hyphens, starting with a letter or digit".into());
        }
        let mut bytes = [0; 100];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("validated ASCII ACP identity")
    }
}
impl fmt::Debug for CustomAcpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomAcpAgent {
    pub id: HarnessId,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Fallback choices when the ACP server does not advertise a model catalog.
    #[serde(default)]
    pub model_ids: Vec<String>,
}
impl CustomAcpAgent {
    /// Settings fields use literal argument lines, not a shell command.
    pub fn from_fields(
        id: &str,
        name: &str,
        command: &str,
        args: &str,
        env: &str,
        models: &str,
    ) -> Result<Self, String> {
        let mut environment = BTreeMap::new();
        for line in env.lines().filter(|line| !line.trim().is_empty()) {
            let (key, value) = line
                .split_once('=')
                .ok_or("Environment entries need NAME=value, one per line")?;
            if environment
                .insert(key.to_owned(), decode_field(value)?)
                .is_some()
            {
                return Err(format!("Duplicate environment variable: {key}"));
            }
        }
        let agent = Self {
            id: HarnessId::try_from(id.trim().to_owned())?,
            name: name.trim().to_owned(),
            command: command.trim().to_owned(),
            args: args.lines().map(decode_field).collect::<Result<_, _>>()?,
            env: environment,
            model_ids: models
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
        };
        agent.validate()?;
        Ok(agent)
    }
    /// Quote only values that cannot be represented literally on one line.
    pub fn argument_fields(&self) -> String {
        self.args
            .iter()
            .map(|value| encode_field(value))
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn environment_fields(&self) -> String {
        self.env
            .iter()
            .map(|(key, value)| format!("{key}={}", encode_field(value)))
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.id, HarnessId::CustomAcp(_)) {
            return Err("Choose a custom acp- identity".into());
        }
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self.name.contains(['\0', '\n', '\r'])
        {
            return Err("Enter a provider name of up to 256 bytes".into());
        }
        if self.command.trim().is_empty()
            || self.command.len() > 8192
            || self.command.contains('\0')
        {
            return Err("Enter an executable path or command name".into());
        }
        if self.args.len() > 128
            || self
                .args
                .iter()
                .any(|arg| arg.len() > 8192 || arg.contains('\0'))
            || self.args.iter().map(String::len).sum::<usize>() > 128 * 1024
        {
            return Err("ACP arguments exceed the launch limit".into());
        }
        if self.env.len() > 128
            || self.env.iter().any(|(key, value)| {
                key.is_empty()
                    || key.len() > 256
                    || key.bytes().enumerate().any(|(index, b)| {
                        !(b.is_ascii_alphabetic() || b == b'_' || (index > 0 && b.is_ascii_digit()))
                    })
                    || matches!(
                        key.as_str(),
                        "ZUSE_CHAT_ID" | "ZUSE_DEVICE_ID" | "ZUSE_IPC_PORT"
                    )
                    || value.len() > 64 * 1024
                    || value.contains('\0')
            })
            || self
                .env
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                > 128 * 1024
        {
            return Err("ACP environment is invalid or exceeds the launch limit".into());
        }
        if self.model_ids.len() > 128
            || self
                .model_ids
                .iter()
                .any(|id| id.trim().is_empty() || id.len() > 512 || id.contains('\0'))
        {
            return Err("ACP model IDs are invalid or exceed the catalog limit".into());
        }
        Ok(())
    }
}
fn encode_field(value: &str) -> String {
    if value.is_empty() || value.contains(['\n', '\r']) || value.starts_with('"') {
        serde_json::to_string(value).expect("string encoding")
    } else {
        value.to_owned()
    }
}
fn decode_field(value: &str) -> Result<String, String> {
    if value.starts_with('"') {
        serde_json::from_str(value).map_err(|_| "Quoted values must be valid JSON strings".into())
    } else {
        Ok(value.to_owned())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomAcpAgents {
    pub revision: u64,
    pub agents: Vec<CustomAcpAgent>,
}

impl From<HarnessId> for String {
    fn from(id: HarnessId) -> Self {
        match id {
            HarnessId::Zuse => "zuse",
            HarnessId::ClaudeCode => "claude-code",
            HarnessId::Codex => "codex",
            HarnessId::Cursor => "cursor",
            HarnessId::Gemini => "gemini",
            HarnessId::Kiro => "kiro",
            HarnessId::Devin => "devin",
            HarnessId::Grok => "grok",
            HarnessId::Hermes => "hermes",
            HarnessId::Pi => "pi",
            HarnessId::Opencode => "opencode",
            HarnessId::Antigravity => "antigravity",
            HarnessId::Mock => "mock",
            HarnessId::CustomAcp(id) => return id.as_str().to_owned(),
        }
        .to_owned()
    }
}
impl TryFrom<String> for HarnessId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_str() {
            "zuse" => Self::Zuse,
            "claude-code" => Self::ClaudeCode,
            "codex" => Self::Codex,
            "cursor" => Self::Cursor,
            "gemini" => Self::Gemini,
            "kiro" => Self::Kiro,
            "devin" => Self::Devin,
            "grok" => Self::Grok,
            "hermes" => Self::Hermes,
            "pi" => Self::Pi,
            "opencode" => Self::Opencode,
            "antigravity" => Self::Antigravity,
            "mock" => Self::Mock,
            _ => Self::CustomAcp(CustomAcpId::parse(&value)?),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_fields_preserve_literal_arguments_and_reject_ambiguous_environment() {
        let agent = CustomAcpAgent::from_fields(
            "acp-local",
            "Local",
            "python3",
            "server.py\n  literal ; $(command)  \n\nlast",
            "TOKEN=a=b\nOTHER=",
            "first\nsecond",
        )
        .unwrap();
        assert_eq!(
            agent.args,
            ["server.py", "  literal ; $(command)  ", "", "last"]
        );
        assert_eq!(agent.env["TOKEN"], "a=b");
        assert_eq!(agent.env["OTHER"], "");
        assert!(
            CustomAcpAgent::from_fields(
                "acp-local",
                "Local",
                "python3",
                "",
                "TOKEN=a\nTOKEN=b",
                ""
            )
            .is_err()
        );
        assert!(
            CustomAcpAgent::from_fields(
                "acp-local",
                "Local",
                "python3",
                "",
                "ZUSE_CHAT_ID=foreign",
                ""
            )
            .is_err()
        );
    }
    #[test]
    fn editing_preserves_empty_and_multiline_launch_values() {
        let original = CustomAcpAgent {
            id: HarnessId::try_from("acp-local".to_string()).unwrap(),
            name: "Local".into(),
            command: "python3".into(),
            args: vec!["".into(), "\"quoted\"".into(), "a\nb".into(), "".into()],
            env: [
                ("TOKEN".into(), "first\nsecond".into()),
                ("QUOTED".into(), "\"literal".into()),
            ]
            .into_iter()
            .collect(),
            model_ids: vec![],
        };
        let edited = CustomAcpAgent::from_fields(
            "acp-local",
            "Local",
            "python3",
            &original.argument_fields(),
            &original.environment_fields(),
            "",
        )
        .unwrap();
        assert_eq!(original, edited);
    }
    #[test]
    fn custom_identity_is_bounded_distinct_and_round_trips_without_changing_builtins() {
        for raw in ["codex", "claude-code", "zuse", "acp-local-agent", "acp-1"] {
            let id: HarnessId = serde_json::from_value(serde_json::json!(raw)).unwrap();
            assert_eq!(serde_json::to_value(id).unwrap(), raw);
        }
        for raw in [
            "",
            "acp-",
            "acp-Upper",
            "acp--bad",
            "acp-../path",
            "unknown",
            &format!("acp-{}", "a".repeat(97)),
        ] {
            assert!(serde_json::from_value::<HarnessId>(serde_json::json!(raw)).is_err());
        }
        assert_ne!(
            HarnessId::try_from("acp-first".to_string()).unwrap(),
            HarnessId::try_from("acp-second".to_string()).unwrap()
        );
    }
}
