//! Which agents can actually run here: the ones that only work signed in
//! need a live login, an API key in the environment, or (Claude) a proxy.
//! The composer hides the rest; Settings still lists them with "Not signed
//! in" so they can be set up.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use zuse_protocol::{ClaudeConnectionMode, HarnessId};

use super::{AgentAccounts, lock};

/// How long a list's view of live logins answers `ListHarnesses`. Every
/// switch, removal and sign-in relists, so this only bounds outside changes
/// (a login made in a terminal).
const FRESH: Duration = Duration::from_secs(20);

/// Agents that refuse to run without a login, and the environment variables
/// their CLIs accept in its place. Everything else (OpenCode's free models,
/// Pi, Kiro, custom ACP agents, Zuse) is never hidden.
fn requirement(harness: HarnessId) -> Option<&'static [&'static str]> {
    Some(match harness {
        HarnessId::ClaudeCode => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
        ],
        HarnessId::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        HarnessId::Cursor => &["CURSOR_API_KEY"],
        HarnessId::Grok => &["XAI_API_KEY", "GROK_API_KEY"],
        HarnessId::Devin => &["DEVIN_API_KEY"],
        HarnessId::Antigravity => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        HarnessId::Hermes => &[],
        _ => return None,
    })
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

#[derive(Default)]
pub(super) struct LoginCache {
    at: Option<Instant>,
    signed_in: HashSet<HarnessId>,
}

impl AgentAccounts {
    /// Record which agents a fresh list found signed in.
    pub(super) fn note_logins(&self, accounts: &[zuse_protocol::AgentAccount]) {
        let mut cache = lock(&self.inner.logins);
        cache.signed_in = accounts
            .iter()
            .filter(|a| a.active)
            .map(|a| a.harness)
            .collect();
        cache.at = Some(Instant::now());
    }

    /// For each agent that needs a login: whether it can run right now.
    pub async fn login_status(&self) -> HashMap<HarnessId, bool> {
        let stale = lock(&self.inner.logins)
            .at
            .is_none_or(|at| at.elapsed() > FRESH);
        if stale && let Err(error) = self.list(false).await {
            tracing::debug!(%error, "login status refresh failed");
        }
        let signed_in = lock(&self.inner.logins).signed_in.clone();
        let proxied = self.claude_connection_mode() == ClaudeConnectionMode::Proxy;
        let all = [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Grok,
            HarnessId::Devin,
            HarnessId::Antigravity,
            HarnessId::Hermes,
        ];
        all.into_iter()
            .filter_map(|harness| {
                let keys = requirement(harness)?;
                let usable = signed_in.contains(&harness)
                    || keys.iter().any(|k| env_set(k))
                    || (harness == HarnessId::ClaudeCode && proxied);
                Some((harness, usable))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_login_bound_agents_are_gated() {
        assert!(requirement(HarnessId::Codex).is_some());
        assert!(requirement(HarnessId::Cursor).is_some());
        assert!(requirement(HarnessId::Hermes).is_some());
        assert!(requirement(HarnessId::Opencode).is_none());
        assert!(requirement(HarnessId::Pi).is_none());
        assert!(requirement(HarnessId::Kiro).is_none());
    }
}
