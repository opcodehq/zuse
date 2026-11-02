//! Portable agent installation guidance shared by native and browser settings.
use crate::HarnessId;
pub fn manual_command(id: HarnessId) -> Option<&'static str> {
    use HarnessId::*;
    Some(match id {
        ClaudeCode => "curl -fsSL https://claude.ai/install.sh | bash",
        Codex => "npm install -g @openai/codex",
        Cursor => "curl https://cursor.com/install -fsS | bash",
        Opencode => "npm install -g @opencode/cli",
        Pi => "npm install -g --ignore-scripts @earendil-works/pi-coding-agent",
        Grok => "npm install -g @xai-official/grok",
        Hermes => "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash",
        Devin => "curl -fsSL https://cli.devin.ai/install.sh | bash",
        Gemini => "npm install -g @google/gemini-cli",
        Zuse | Kiro | Antigravity | Mock | CustomAcp(_) => return None,
    })
}
