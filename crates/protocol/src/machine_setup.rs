//! Setup chats: a cloud chat whose agent configures the account's machine
//! image. The agent reads the image configuration, tests a setup script on
//! its own computer and proposes a new configuration (setup script and
//! repository list); the owner applies it, which saves the same recipe the
//! settings page edits and queues the normal image rebuild.
use serde::{Deserialize, Serialize};

/// [`crate::cloud::CloudWorkspaceCreateRequest::purpose`] of a setup chat.
pub const PURPOSE: &str = "machine-setup";
/// Bootstrap (and `request_config`) key marking a setup computer.
pub const BOOTSTRAP_FLAG: &str = "machineSetup";
/// Engine RPC the Zuse MCP server calls for the image tools.
pub const METHOD: &str = "MachineSetup";
/// Environment flag the runtime gives the Zuse MCP server in a setup chat.
pub const MCP_ENV: &str = "ZUSE_MACHINE_SETUP";

/// Repositories a proposal may list.
pub const MAX_REPOSITORIES: usize = 50;
/// Longest proposal note, in bytes.
pub const MAX_NOTES: usize = 4000;
/// `test_setup_script` deadline when none is asked for, and its ceiling.
pub const DEFAULT_TEST_TIMEOUT_SECS: u32 = 600;
pub const MAX_TEST_TIMEOUT_SECS: u32 = 1800;

/// What the agent in a setup chat is told, on top of the account's own
/// instructions.
pub const INSTRUCTIONS: &str = "\
# You are configuring this account's cloud machine image

This chat runs on a setup computer started from the account's current machine image. \
The user wants to change what every new cloud computer has before a repository is checked out.

- Express every change as image configuration, never as ad-hoc state on this computer: \
a setup script (bash, runs once per image build as `zuse` with sudo) and the list of repositories \
baked into the image. Anything you install here by hand is lost; only the script is rebuilt.
- Start with `get_machine_image_config`. Keep the existing script's steps unless the user asks to drop them.
- Write the script idempotent and non-interactive: `set -euo pipefail`, `apt-get install -y`, \
`DEBIAN_FRONTEND=noninteractive`, installers with their `-y`/unattended flags, and checks such as \
`command -v cargo || ...` so a second run is harmless. Install user tools for `zuse` (e.g. rustup, bun, uv) \
and make them available on PATH for login shells (e.g. a file in /etc/profile.d).
- Run `test_setup_script` with the full proposed script before proposing, and fix failures.
- Never put secrets, tokens or passwords in the script; they would be baked into the image. \
Credentials are delivered per chat. Do not touch /usr/local/bin/zuse-cloud-runtime, /usr/local/lib/zuse \
or the zuse-runtime-supervisor service.
- Repositories: list the full set the image should carry as `github.com/owner/name`. Add only \
repositories the user asks for; they must be reachable through the user's GitHub connection, which Zuse \
checks when the user applies.
- Then call `propose_machine_image_config`. The user reviews the change in this chat and chooses \
Apply & rebuild or Discard; do not claim it is applied before they do.
";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    /// The current setup script, repositories and image state.
    Get,
    /// Store a proposal for the owner to review.
    #[serde(rename_all = "camelCase")]
    Propose {
        setup_script: String,
        repositories: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
}

/// One repository the image carries.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageRepository {
    /// `github.com/owner/name`.
    pub identity: String,
    #[serde(default)]
    pub default_branch: String,
    /// `baked`, `skipped` or absent when the image has not been built with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_state: Option<String>,
}

/// What `get_machine_image_config` returns.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageConfig {
    pub revision: i64,
    pub setup_script: String,
    pub repositories: Vec<ImageRepository>,
    /// `none`, `queued`, `building`, `ready` or `failed`.
    pub image_state: String,
}

/// A configuration the agent proposed, as the owner reviews it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub proposal_id: String,
    pub chat_id: String,
    /// Recipe revision the proposal was made against.
    pub base_revision: i64,
    pub current_setup_script: String,
    pub setup_script: String,
    pub current_repositories: Vec<String>,
    pub repositories: Vec<String>,
    #[serde(default)]
    pub notes: String,
    /// `proposed`, `applied`, `discarded` or `superseded`.
    pub state: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    /// The recipe revision applying it saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_revision: Option<i64>,
}

impl Proposal {
    /// Repositories the proposal adds and removes, in its order.
    pub fn repository_changes(&self) -> (Vec<&str>, Vec<&str>) {
        let added = self
            .repositories
            .iter()
            .filter(|r| !self.current_repositories.contains(r))
            .map(String::as_str)
            .collect();
        let removed = self
            .current_repositories
            .iter()
            .filter(|r| !self.repositories.contains(r))
            .map(String::as_str)
            .collect();
        (added, removed)
    }
    pub fn script_changed(&self) -> bool {
        self.setup_script.trim() != self.current_setup_script.trim()
    }
}

/// A proposal with the account's machine, for the chat card.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub proposal: Proposal,
    pub machine: crate::cloud_computers::MachineVersion,
}

/// A proposal ID: `msp_` and 32 lowercase hex digits.
pub fn proposal_id_valid(id: &str) -> bool {
    id.strip_prefix("msp_").is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `github.com/owner/name` for `owner/name`, a GitHub URL or an identity;
/// `None` for anything else.
pub fn repository_identity(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(raw);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest
        .strip_prefix("github.com/")
        .or_else(|| rest.strip_prefix("git@github.com:"))
        .unwrap_or(rest);
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let parts: Vec<&str> = rest.split('/').collect();
    let valid = |part: &&str| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            && *part != "."
            && *part != ".."
    };
    (parts.len() == 2 && parts.iter().all(valid))
        .then(|| format!("github.com/{}/{}", parts[0], parts[1]).to_lowercase())
}

/// Something in `script` that looks like a credential: a known token prefix
/// or a secret-named assignment with a literal value.
pub fn secret_in_script(script: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "sk-ant-",
        "sk-proj-",
        "AKIA",
        "npm_",
    ];
    const KEYS: &[&str] = &[
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "APIKEY",
        "_AUTHTOKEN",
    ];
    script.lines().any(|line| {
        let words = line.split(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ':' | '`' | '(' | ')')
        });
        if words.clone().any(|word| {
            PREFIXES
                .iter()
                .any(|p| word.starts_with(p) && word.len() >= p.len() + 16)
        }) {
            return true;
        }
        // `export NPM_TOKEN=abc123…` but not `NPM_TOKEN="$NPM_TOKEN"` or `${X}`.
        let upper = line.to_ascii_uppercase();
        let Some((name, value)) = upper.split_once('=') else {
            return false;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        KEYS.iter().any(|key| name.contains(key))
            && value.len() >= 12
            && !value.contains('$')
            && !value.contains(' ')
    })
}

/// Checks the agent's proposal before it is stored.
pub fn validate_proposal(
    setup_script: &str,
    repositories: &[String],
    notes: Option<&str>,
) -> Result<Vec<String>, &'static str> {
    let recipe = crate::cloud_computers::MachineRecipe {
        setup_script: setup_script.to_owned(),
        ..Default::default()
    };
    recipe.validate()?;
    if setup_script.contains('\0') {
        return Err("invalid_machine_recipe");
    }
    if secret_in_script(setup_script) {
        return Err("machine_setup_script_contains_secret");
    }
    if notes.is_some_and(|n| n.len() > MAX_NOTES) {
        return Err("machine_setup_notes_too_long");
    }
    if repositories.len() > MAX_REPOSITORIES {
        return Err("machine_setup_too_many_repositories");
    }
    let mut out: Vec<String> = Vec::new();
    for raw in repositories {
        let identity = repository_identity(raw).ok_or("machine_setup_repository_invalid")?;
        if !out.contains(&identity) {
            out.push(identity);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_normalize_to_one_identity() {
        for raw in [
            "Opcode/Zuse",
            "github.com/opcode/zuse",
            "https://github.com/opcode/zuse.git",
            "git@github.com:opcode/zuse.git",
            "https://www.github.com/Opcode/Zuse/",
        ] {
            assert_eq!(
                repository_identity(raw).as_deref(),
                Some("github.com/opcode/zuse"),
                "{raw}"
            );
        }
        for raw in [
            "zuse",
            "a/b/c",
            "gitlab.com/a/b",
            "../x",
            "a/..",
            "a b/c",
            "",
        ] {
            assert_eq!(repository_identity(raw), None, "{raw}");
        }
    }

    #[test]
    fn scripts_with_literal_credentials_are_refused() {
        assert!(secret_in_script(
            "export NPM_TOKEN=npm_abcdefghijklmnopqrstuvwxyz"
        ));
        assert!(secret_in_script("GITHUB_TOKEN=abcdef1234567890"));
        assert!(secret_in_script(
            "curl -H 'Authorization: token ghp_aaaaaaaaaaaaaaaaaaaaaaaa'"
        ));
        assert!(!secret_in_script(
            "set -euo pipefail\ncommand -v cargo || curl -fsSL https://sh.rustup.rs | sh -s -- -y"
        ));
        assert!(!secret_in_script("NPM_TOKEN=\"${NPM_TOKEN:-}\""));
        assert_eq!(
            validate_proposal("export API_KEY=abcdefghijklmnop", &[], None),
            Err("machine_setup_script_contains_secret")
        );
    }

    #[test]
    fn proposals_dedupe_repositories_and_report_changes() {
        let repos = validate_proposal(
            "sudo apt-get install -y jq",
            &["a/b".into(), "github.com/A/B".into(), "c/d".into()],
            Some("Adds jq"),
        )
        .unwrap();
        assert_eq!(repos, ["github.com/a/b", "github.com/c/d"]);
        assert_eq!(
            validate_proposal("true", &["nope".into()], None),
            Err("machine_setup_repository_invalid")
        );
        let proposal = Proposal {
            current_repositories: vec!["github.com/a/b".into(), "github.com/x/y".into()],
            repositories: repos,
            current_setup_script: "true\n".into(),
            setup_script: "true".into(),
            ..Default::default()
        };
        assert_eq!(
            proposal.repository_changes(),
            (vec!["github.com/c/d"], vec!["github.com/x/y"])
        );
        assert!(!proposal.script_changed());
    }

    #[test]
    fn requests_round_trip_in_the_wire_shape() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "action":"propose","setupScript":"true","repositories":["a/b"],"notes":"n"
        }))
        .unwrap();
        assert_eq!(
            request,
            Request::Propose {
                setup_script: "true".into(),
                repositories: vec!["a/b".into()],
                notes: Some("n".into())
            }
        );
        assert_eq!(
            serde_json::to_value(Request::Get).unwrap(),
            serde_json::json!({"action":"get"})
        );
    }
}
