//! A selected account may have no optional display label.
pub(super) fn selected_label(label: Option<&str>, scope: Option<&str>, agent: &str) -> String {
    label
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{} · {agent}",
                if scope == Some("organization") {
                    "Organization"
                } else {
                    "Personal"
                }
            )
        })
}

#[cfg(test)]
mod tests {
    use super::selected_label;
    use std::prelude::v1::test;
    #[test]
    fn missing_optional_label_keeps_the_selected_personal_agent_visible() {
        assert_eq!(selected_label(None, None, "codex"), "Personal · codex");
    }
    #[test]
    fn blank_organization_label_uses_the_same_scope_and_agent_fallback_as_picker() {
        assert_eq!(
            selected_label(Some("  "), Some("organization"), "claude"),
            "Organization · claude"
        );
    }
    #[test]
    fn existing_label_is_preserved_without_selecting_a_different_account() {
        assert_eq!(
            selected_label(Some("Work profile"), Some("personal"), "codex"),
            "Work profile"
        );
    }
}
