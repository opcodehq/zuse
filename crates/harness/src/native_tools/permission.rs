use crate::{PermissionMode, ToolCategory};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMode {
    Auto,
    ApprovalRequired,
    AutoAcceptEdits,
    AutoAcceptEditsAndBash,
    FullAccess,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Prompt,
    Deny,
}
pub fn decide(
    runtime: RuntimeMode,
    mode: PermissionMode,
    category: ToolCategory,
    sensitive: bool,
    can_prompt: bool,
) -> Verdict {
    let prompt = if can_prompt {
        Verdict::Prompt
    } else {
        Verdict::Deny
    };
    if category == ToolCategory::ExitPlan {
        return prompt;
    }
    if mode == PermissionMode::Plan {
        return if category != ToolCategory::Read {
            Verdict::Deny
        } else if sensitive {
            prompt
        } else {
            Verdict::Allow
        };
    }
    if sensitive {
        return prompt;
    }
    if category == ToolCategory::Read {
        return Verdict::Allow;
    }
    if runtime != RuntimeMode::Auto
        && mode == PermissionMode::AcceptEdits
        && category == ToolCategory::Edit
    {
        return Verdict::Allow;
    }
    match runtime {
        RuntimeMode::FullAccess => Verdict::Allow,
        RuntimeMode::AutoAcceptEdits if category == ToolCategory::Edit => Verdict::Allow,
        RuntimeMode::AutoAcceptEditsAndBash
            if matches!(category, ToolCategory::Edit | ToolCategory::Execute) =>
        {
            Verdict::Allow
        }
        _ => prompt,
    }
}
pub fn sensitive(path: &Path) -> bool {
    path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy();
        let lower = name.to_ascii_lowercase();
        name == ".env"
            || name.starts_with(".env.")
            || matches!(name.as_ref(), ".aws" | ".ssh" | ".netrc" | ".pgpass")
            || lower == "credentials"
            || lower.starts_with("credentials.")
            || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
                .iter()
                .any(|key| name == *key || name == format!("{key}.pub"))
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|extension| lower.ends_with(extension))
    })
}
