//! Native validation for the shared desktop keybinding file.
pub use zuse_protocol::keybindings::{Command, Rule, evaluate_when, normalize_key};
pub fn import(value: &serde_json::Value) -> Result<Vec<Rule>, String> {
    let rules = zuse_protocol::keybindings::import(value)?;
    for rule in &rules {
        if !rule.key.is_empty()
            && gpui::Keystroke::parse(&super::platform_combo(&rule.key)).is_err()
        {
            return Err("A keyboard shortcut has an unsupported key combination.".into());
        }
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_resets_electron_commands_preserves_native_extras_and_keeps_order() {
        use crate::settings::{KeymapConfig, ShortcutId};
        let mut keymap = KeymapConfig::default();
        keymap.new_session = "mod-alt-8".into();
        keymap.toggle_dictation = "mod-shift-v".into();
        keymap.import_desktop_rules(
            import(&serde_json::json!({"schemaVersion":1,"rules":[
                {"key":"mod+alt+t","command":"new-tab"},
                {"key":"mod+shift+t","command":"new-tab"},
                {"key":"","command":"close-tab"}
            ]}))
            .unwrap(),
        );
        assert_eq!(keymap.get(ShortcutId::Desktop(Command::NewChat)), "mod-n");
        assert_eq!(keymap.toggle_dictation, "mod-shift-v");
        let rules = keymap.desktop_bindings();
        assert_eq!(
            rules
                .iter()
                .filter(|rule| rule.command == Command::NewTab)
                .map(|rule| rule.key.as_str())
                .collect::<Vec<_>>(),
            vec!["mod-alt-t", "mod-shift-t"]
        );
        assert_eq!(keymap.get(ShortcutId::Desktop(Command::CloseTab)), "");
        let serialized = serde_json::to_string(&keymap).unwrap();
        let restored: KeymapConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored, keymap);
    }
}
