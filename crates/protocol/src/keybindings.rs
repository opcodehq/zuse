//! Keyboard command contract shared with the Electron desktop app.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    #[serde(rename = "new-chat")]
    NewChat,
    #[serde(rename = "new-chat-without-project")]
    NewChatWithoutProject,
    #[serde(rename = "open-project")]
    OpenProject,
    #[serde(rename = "settings")]
    Settings,
    #[serde(rename = "close-tab")]
    CloseTab,
    #[serde(rename = "toggle-left-sidebar")]
    ToggleLeftSidebar,
    #[serde(rename = "toggle-right-sidebar")]
    ToggleRightSidebar,
    #[serde(rename = "toggle-terminal")]
    ToggleTerminal,
    #[serde(rename = "focus-composer")]
    FocusComposer,
    #[serde(rename = "next-tab")]
    NextTab,
    #[serde(rename = "prev-tab")]
    PrevTab,
    #[serde(rename = "select-tab-1")]
    SelectTab1,
    #[serde(rename = "select-tab-2")]
    SelectTab2,
    #[serde(rename = "select-tab-3")]
    SelectTab3,
    #[serde(rename = "select-tab-4")]
    SelectTab4,
    #[serde(rename = "select-tab-5")]
    SelectTab5,
    #[serde(rename = "select-tab-6")]
    SelectTab6,
    #[serde(rename = "select-tab-7")]
    SelectTab7,
    #[serde(rename = "select-tab-8")]
    SelectTab8,
    #[serde(rename = "select-last-tab")]
    SelectLastTab,
    #[serde(rename = "new-tab")]
    NewTab,
    #[serde(rename = "next-chat")]
    NextChat,
    #[serde(rename = "prev-chat")]
    PrevChat,
    #[serde(rename = "next-panel")]
    NextPanel,
    #[serde(rename = "prev-panel")]
    PrevPanel,
    #[serde(rename = "focus-next-pane")]
    FocusNextPane,
    #[serde(rename = "focus-prev-pane")]
    FocusPrevPane,
    #[serde(rename = "open-chat-switcher")]
    OpenChatSwitcher,
    #[serde(rename = "search-files")]
    SearchFiles,
    #[serde(rename = "show-shortcuts")]
    ShowShortcuts,
    #[serde(rename = "find-in-chat")]
    FindInChat,
    #[serde(rename = "open-model-picker")]
    OpenModelPicker,
    #[serde(rename = "cycle-reasoning-effort")]
    CycleReasoningEffort,
    #[serde(rename = "toggle-fast-mode")]
    ToggleFastMode,
    #[serde(rename = "toggle-tool-details")]
    ToggleToolDetails,
    #[serde(rename = "toggle-task-list")]
    ToggleTaskList,
    #[serde(rename = "stop-background-tasks")]
    StopBackgroundTasks,
    #[serde(rename = "composer.submit")]
    ComposerSubmit,
    #[serde(rename = "composer.newline")]
    ComposerNewline,
    #[serde(rename = "composer.forceSubmit")]
    ComposerForceSubmit,
    #[serde(rename = "composer.togglePlanMode")]
    ComposerTogglePlanMode,
    #[serde(rename = "composer.previousMessage")]
    ComposerPreviousMessage,
    #[serde(rename = "composer.nextMessage")]
    ComposerNextMessage,
    #[serde(rename = "composer.queueSubmit")]
    ComposerQueueSubmit,
    #[serde(rename = "composer.deleteToStart")]
    ComposerDeleteToStart,
    #[serde(rename = "composer.deleteToEnd")]
    ComposerDeleteToEnd,
    #[serde(rename = "composer.deleteWord")]
    ComposerDeleteWord,
    #[serde(rename = "composer.interrupt")]
    ComposerInterrupt,
    #[serde(rename = "composer.yank")]
    ComposerYank,
    #[serde(rename = "composer.yankPop")]
    ComposerYankPop,
    #[serde(rename = "composer.stash")]
    ComposerStash,
    #[serde(rename = "composer.searchHistory")]
    ComposerSearchHistory,
    #[serde(rename = "editor.save")]
    EditorSave,
    #[serde(rename = "editor.annotate")]
    EditorAnnotate,
}
impl Command {
    pub const ALL: [Self; 54] = [
        Self::NewChat,
        Self::NewChatWithoutProject,
        Self::OpenProject,
        Self::Settings,
        Self::CloseTab,
        Self::ToggleLeftSidebar,
        Self::ToggleRightSidebar,
        Self::ToggleTerminal,
        Self::FocusComposer,
        Self::NextTab,
        Self::PrevTab,
        Self::SelectTab1,
        Self::SelectTab2,
        Self::SelectTab3,
        Self::SelectTab4,
        Self::SelectTab5,
        Self::SelectTab6,
        Self::SelectTab7,
        Self::SelectTab8,
        Self::SelectLastTab,
        Self::NewTab,
        Self::NextChat,
        Self::PrevChat,
        Self::NextPanel,
        Self::PrevPanel,
        Self::FocusNextPane,
        Self::FocusPrevPane,
        Self::OpenChatSwitcher,
        Self::SearchFiles,
        Self::ShowShortcuts,
        Self::FindInChat,
        Self::OpenModelPicker,
        Self::CycleReasoningEffort,
        Self::ToggleFastMode,
        Self::ToggleToolDetails,
        Self::ToggleTaskList,
        Self::StopBackgroundTasks,
        Self::ComposerSubmit,
        Self::ComposerNewline,
        Self::ComposerForceSubmit,
        Self::ComposerTogglePlanMode,
        Self::ComposerPreviousMessage,
        Self::ComposerNextMessage,
        Self::ComposerQueueSubmit,
        Self::ComposerDeleteToStart,
        Self::ComposerDeleteToEnd,
        Self::ComposerDeleteWord,
        Self::ComposerInterrupt,
        Self::ComposerYank,
        Self::ComposerYankPop,
        Self::ComposerStash,
        Self::ComposerSearchHistory,
        Self::EditorSave,
        Self::EditorAnnotate,
    ];
    pub fn default_combo(self) -> &'static str {
        self.default_combo_on(cfg!(target_os = "macos"))
    }
    /// The default for an explicit platform. Control+letter is free on macOS,
    /// where Cmd is the primary modifier; elsewhere it would collide with the
    /// Ctrl-as-primary shortcuts, so those defaults add Shift.
    pub fn default_combo_on(self, mac: bool) -> &'static str {
        match self {
            Self::ToggleToolDetails if mac => "ctrl-o",
            Self::ToggleToolDetails => "mod-shift-o",
            Self::ToggleTaskList if mac => "ctrl-t",
            Self::ToggleTaskList => "mod-shift-t",
            Self::NewChat => "mod-n",
            Self::NewChatWithoutProject => "mod-alt-n",
            Self::OpenProject => "mod-o",
            Self::Settings => "mod-,",
            Self::CloseTab => "mod-w",
            Self::ToggleLeftSidebar => "mod-b",
            Self::ToggleRightSidebar => "mod-alt-b",
            Self::ToggleTerminal => "mod-j",
            Self::FocusComposer => "mod-l",
            Self::NextTab => "mod-shift-]",
            Self::PrevTab => "mod-shift-[",
            Self::SelectTab1 => "mod-1",
            Self::SelectTab2 => "mod-2",
            Self::SelectTab3 => "mod-3",
            Self::SelectTab4 => "mod-4",
            Self::SelectTab5 => "mod-5",
            Self::SelectTab6 => "mod-6",
            Self::SelectTab7 => "mod-7",
            Self::SelectTab8 => "mod-8",
            Self::SelectLastTab => "mod-9",
            Self::NewTab => "mod-t",
            Self::NextChat => "ctrl-tab",
            Self::PrevChat => "ctrl-shift-tab",
            Self::NextPanel => "mod-alt-]",
            Self::PrevPanel => "mod-alt-[",
            Self::FocusNextPane => "ctrl-`",
            Self::FocusPrevPane => "ctrl-shift-`",
            Self::OpenChatSwitcher => "mod-k",
            Self::SearchFiles => "mod-p",
            Self::ShowShortcuts => "mod-/",
            Self::FindInChat => "mod-f",
            Self::OpenModelPicker => "alt-p",
            Self::CycleReasoningEffort => "alt-t",
            Self::ToggleFastMode => "alt-o",
            Self::StopBackgroundTasks => "mod-shift-k",
            Self::ComposerSubmit => "enter",
            Self::ComposerNewline => "shift-enter",
            Self::ComposerForceSubmit => "mod-enter",
            Self::ComposerTogglePlanMode => "shift-tab",
            Self::ComposerPreviousMessage => "up",
            Self::ComposerNextMessage => "down",
            Self::ComposerQueueSubmit => "alt-enter",
            Self::ComposerDeleteToStart => "ctrl-u",
            Self::ComposerDeleteToEnd => "ctrl-k",
            Self::ComposerDeleteWord => "ctrl-backspace",
            Self::ComposerInterrupt => "escape",
            Self::ComposerYank => "ctrl-y",
            Self::ComposerYankPop => "alt-y",
            Self::ComposerStash => "ctrl-s",
            Self::ComposerSearchHistory => "ctrl-r",
            Self::EditorSave => "mod-s",
            Self::EditorAnnotate => "mod-shift-a",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::NewChat => "New chat",
            Self::NewChatWithoutProject => "New chat without a project",
            Self::OpenProject => "Open project",
            Self::Settings => "Settings",
            Self::CloseTab => "Close tab",
            Self::ToggleLeftSidebar => "Toggle projects panel",
            Self::ToggleRightSidebar => "Toggle files sidebar",
            Self::ToggleTerminal => "Toggle terminal",
            Self::FocusComposer => "Focus composer",
            Self::NextTab => "Next tab",
            Self::PrevTab => "Previous tab",
            Self::SelectTab1 => "Go to tab 1",
            Self::SelectTab2 => "Go to tab 2",
            Self::SelectTab3 => "Go to tab 3",
            Self::SelectTab4 => "Go to tab 4",
            Self::SelectTab5 => "Go to tab 5",
            Self::SelectTab6 => "Go to tab 6",
            Self::SelectTab7 => "Go to tab 7",
            Self::SelectTab8 => "Go to tab 8",
            Self::SelectLastTab => "Go to last tab",
            Self::NewTab => "New tab",
            Self::NextChat => "Next chat",
            Self::PrevChat => "Previous chat",
            Self::NextPanel => "Next panel",
            Self::PrevPanel => "Previous panel",
            Self::FocusNextPane => "Focus next pane",
            Self::FocusPrevPane => "Focus previous pane",
            Self::OpenChatSwitcher => "Open chat switcher",
            Self::SearchFiles => "Search files",
            Self::ShowShortcuts => "Show keyboard shortcuts",
            Self::FindInChat => "Find in chat",
            Self::OpenModelPicker => "Switch model",
            Self::CycleReasoningEffort => "Change reasoning effort",
            Self::ToggleFastMode => "Toggle fast mode",
            Self::ToggleToolDetails => "Expand or collapse tool details",
            Self::ToggleTaskList => "Show or hide task list",
            Self::StopBackgroundTasks => "Stop background tasks",
            Self::ComposerSubmit => "Send message",
            Self::ComposerNewline => "Insert newline",
            Self::ComposerForceSubmit => "Send message now",
            Self::ComposerTogglePlanMode => "Toggle plan mode",
            Self::ComposerPreviousMessage => "Previous sent message",
            Self::ComposerNextMessage => "Next sent message",
            Self::ComposerQueueSubmit => "Queue message",
            Self::ComposerDeleteToStart => "Delete to line start",
            Self::ComposerDeleteToEnd => "Delete to line end",
            Self::ComposerDeleteWord => "Delete previous word",
            Self::ComposerInterrupt => "Stop agent (press twice)",
            Self::ComposerYank => "Paste deleted text",
            Self::ComposerYankPop => "Cycle deleted text",
            Self::ComposerStash => "Stash draft",
            Self::ComposerSearchHistory => "Search sent messages",
            Self::EditorSave => "Save file",
            Self::EditorAnnotate => "Annotate selection",
        }
    }
    pub fn group(self) -> &'static str {
        match self {
            Self::ComposerSubmit
            | Self::ComposerNewline
            | Self::ComposerForceSubmit
            | Self::ComposerTogglePlanMode
            | Self::ComposerPreviousMessage
            | Self::ComposerNextMessage
            | Self::ComposerQueueSubmit
            | Self::ComposerDeleteToStart
            | Self::ComposerDeleteToEnd
            | Self::ComposerDeleteWord
            | Self::ComposerInterrupt
            | Self::ComposerYank
            | Self::ComposerYankPop
            | Self::ComposerStash
            | Self::ComposerSearchHistory => "Composer",
            Self::EditorSave | Self::EditorAnnotate | Self::SearchFiles => "Files",
            Self::NextTab
            | Self::PrevTab
            | Self::SelectTab1
            | Self::SelectTab2
            | Self::SelectTab3
            | Self::SelectTab4
            | Self::SelectTab5
            | Self::SelectTab6
            | Self::SelectTab7
            | Self::SelectTab8
            | Self::SelectLastTab
            | Self::NextChat
            | Self::PrevChat
            | Self::NextPanel
            | Self::PrevPanel
            | Self::FocusNextPane
            | Self::FocusPrevPane => "Navigation",
            _ => "Application",
        }
    }
    pub fn context(self) -> Option<&'static str> {
        match self {
            Self::ComposerSubmit
            | Self::ComposerNewline
            | Self::ComposerForceSubmit
            | Self::ComposerTogglePlanMode
            | Self::ComposerPreviousMessage
            | Self::ComposerNextMessage
            | Self::ComposerQueueSubmit
            | Self::ComposerDeleteToStart
            | Self::ComposerDeleteToEnd
            | Self::ComposerDeleteWord
            | Self::ComposerInterrupt
            | Self::ComposerYank
            | Self::ComposerYankPop
            | Self::ComposerStash
            | Self::ComposerSearchHistory => Some("MessageComposer"),
            Self::EditorSave | Self::EditorAnnotate => Some("Input"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub key: String,
    pub command: Command,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Convert the Electron key vocabulary without losing literal modifiers or punctuation.
pub fn normalize_key(input: &str) -> Option<String> {
    let input = input.trim().to_lowercase();
    if input.is_empty() {
        return Some(String::new());
    }
    if input == "+" {
        return Some("+".into());
    }
    let mut parts: Vec<&str> = input.split('+').collect();
    if input.ends_with("++") {
        parts.pop();
        parts.pop();
        parts.push("+");
    }
    let mut modifiers = [false; 5];
    let mut base = None;
    for token in parts {
        match token {
            "mod" => modifiers[0] = true,
            "cmd" | "meta" | "command" => modifiers[1] = true,
            "ctrl" | "control" => modifiers[2] = true,
            "alt" | "option" | "opt" => modifiers[3] = true,
            "shift" => modifiers[4] = true,
            "" => return None,
            token if base.is_none() => {
                base = Some(match token {
                    "esc" => "escape",
                    "return" => "enter",
                    "ins" => "insert",
                    "del" => "delete",
                    "pgup" => "pageup",
                    "pgdn" | "pagedn" => "pagedown",
                    "spc" | " " => "space",
                    "plus" => "+",
                    other => other,
                })
            }
            _ => return None,
        }
    }
    let base = base?;
    let mut out = Vec::new();
    for (index, token) in ["mod", "cmd", "ctrl", "alt", "shift"]
        .into_iter()
        .enumerate()
    {
        if modifiers[index] {
            out.push(token);
        }
    }
    out.push(base);
    Some(out.join("-"))
}

pub fn import(value: &serde_json::Value) -> Result<Vec<Rule>, String> {
    if value["schemaVersion"] != 1 {
        return Err("This keyboard shortcuts file uses an unsupported version.".into());
    }
    let raw = value["rules"]
        .as_array()
        .ok_or("Keyboard shortcut rules are missing.")?;
    if raw.len() > 256 {
        return Err("The keyboard shortcuts file exceeds 256 rules.".into());
    }
    raw.iter()
        .map(|value| {
            let mut rule: Rule = serde_json::from_value(value.clone())
                .map_err(|_| "A keyboard shortcut has an unknown command or invalid rule.")?;
            rule.key = normalize_key(&rule.key)
                .ok_or("A keyboard shortcut has an invalid key combination.")?;
            // Application when clauses use an empty context in Electron as well.
            if let Some(when) = &rule.when {
                evaluate_when(when)?;
            }
            Ok(rule)
        })
        .collect()
}

/// Electron's application dispatcher evaluates when clauses against an empty context.
pub fn evaluate_when(input: &str) -> Result<bool, String> {
    struct Parser<'a> {
        rest: &'a str,
        depth: usize,
    }
    impl Parser<'_> {
        fn take(&mut self, text: &str) -> bool {
            self.rest = self.rest.trim_start();
            if let Some(rest) = self.rest.strip_prefix(text) {
                self.rest = rest;
                true
            } else {
                false
            }
        }
        fn atom(&mut self) -> Result<bool, String> {
            self.depth += 1;
            if self.depth > 64 {
                return Err("Keyboard shortcut condition is too deeply nested.".into());
            }
            let value = if self.take("!") {
                !self.atom()?
            } else if self.take("(") {
                let v = self.or()?;
                if !self.take(")") {
                    return Err(
                        "Keyboard shortcut condition is missing a closing parenthesis.".into(),
                    );
                }
                v
            } else {
                self.rest = self.rest.trim_start();
                let len = self
                    .rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .count();
                if len == 0 || self.rest.starts_with(|c: char| c.is_ascii_digit()) {
                    return Err("Keyboard shortcut condition is invalid.".into());
                }
                self.rest = &self.rest[len..];
                false
            };
            self.depth -= 1;
            Ok(value)
        }
        fn and(&mut self) -> Result<bool, String> {
            let mut v = self.atom()?;
            while self.take("&&") {
                let right = self.atom()?;
                v &= right;
            }
            Ok(v)
        }
        fn or(&mut self) -> Result<bool, String> {
            let mut v = self.and()?;
            while self.take("||") {
                let right = self.and()?;
                v |= right;
            }
            Ok(v)
        }
    }
    if input.len() > 4096 {
        return Err("Keyboard shortcut condition is too long.".into());
    }
    if input.trim().is_empty() {
        return Ok(true);
    }
    let mut parser = Parser {
        rest: input,
        depth: 0,
    };
    let value = parser.or()?;
    if !parser.rest.trim().is_empty() {
        return Err("Keyboard shortcut condition contains unexpected input.".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_desktop_defaults_round_trip_through_the_import_contract() {
        let rules:Vec<_>=Command::ALL.into_iter().map(|command|serde_json::json!({"key":command.default_combo().replace('-',"+"),"command":command})).collect();
        let parsed = import(&serde_json::json!({"schemaVersion":1,"rules":rules})).unwrap();
        assert_eq!(parsed.len(), Command::ALL.len());
        for (rule, command) in parsed.iter().zip(Command::ALL) {
            assert_eq!(rule.command, command);
            assert_eq!(rule.key, command.default_combo());
        }
    }
    #[test]
    fn imported_modifiers_punctuation_conditions_and_disabled_rules_survive() {
        let rules=import(&serde_json::json!({"schemaVersion":1,"rules":[
            {"key":"command+alt+shift+]","command":"next-tab"},
            {"key":"ctrl+`","command":"focus-next-pane","when":"!settingsOpen && (!composerFocus || editorFocus)"},
            {"key":"mod++","command":"new-tab"},
            {"key":"","command":"close-tab"}
        ]})).unwrap();
        assert_eq!(rules[0].key, "cmd-alt-shift-]");
        assert_eq!(rules[1].key, "ctrl-`");
        assert_eq!(rules[2].key, "mod-+");
        assert_eq!(rules[3].key, "");
        assert!(evaluate_when(rules[1].when.as_deref().unwrap()).unwrap());
        assert!(!evaluate_when("settingsOpen || composerFocus").unwrap());
        for invalid in ["foo &&", "foo | bar", "(foo", "1foo"] {
            assert!(evaluate_when(invalid).is_err());
        }
    }
    #[test]
    fn composer_message_commands_keep_their_wire_names_and_scope() {
        for (name, command, key) in [
            (
                "composer.previousMessage",
                Command::ComposerPreviousMessage,
                "up",
            ),
            ("composer.nextMessage", Command::ComposerNextMessage, "down"),
            (
                "composer.queueSubmit",
                Command::ComposerQueueSubmit,
                "alt-enter",
            ),
            (
                "composer.deleteToStart",
                Command::ComposerDeleteToStart,
                "ctrl-u",
            ),
            (
                "composer.deleteToEnd",
                Command::ComposerDeleteToEnd,
                "ctrl-k",
            ),
            (
                "composer.deleteWord",
                Command::ComposerDeleteWord,
                "ctrl-backspace",
            ),
            ("composer.interrupt", Command::ComposerInterrupt, "escape"),
            ("composer.yank", Command::ComposerYank, "ctrl-y"),
            ("composer.yankPop", Command::ComposerYankPop, "alt-y"),
            ("composer.stash", Command::ComposerStash, "ctrl-s"),
            (
                "composer.searchHistory",
                Command::ComposerSearchHistory,
                "ctrl-r",
            ),
        ] {
            assert_eq!(serde_json::to_value(command).unwrap(), name);
            assert_eq!(command.default_combo(), key);
            assert_eq!(command.context(), Some("MessageComposer"));
            assert_eq!(command.group(), "Composer");
        }
    }
    #[test]
    fn every_command_has_a_unique_wire_name_and_is_importable() {
        let names: std::collections::HashSet<_> = Command::ALL
            .iter()
            .map(|command| serde_json::to_value(command).unwrap())
            .collect();
        assert_eq!(names.len(), Command::ALL.len());
        let rules = import(&serde_json::json!({"schemaVersion":1,"rules":[
            {"key":"mod+shift+/","command":"show-shortcuts"},
            {"key":"alt+m","command":"open-model-picker"},
            {"key":"ctrl+alt+k","command":"stop-background-tasks"}
        ]}))
        .unwrap();
        assert_eq!(rules[0].command, Command::ShowShortcuts);
        assert_eq!(rules[1].command, Command::OpenModelPicker);
        assert_eq!(rules[2].command, Command::StopBackgroundTasks);
    }
    #[test]
    fn chat_defaults_differ_by_platform_only_where_ctrl_would_collide() {
        assert_eq!(Command::ToggleToolDetails.default_combo_on(true), "ctrl-o");
        assert_eq!(
            Command::ToggleToolDetails.default_combo_on(false),
            "mod-shift-o"
        );
        assert_eq!(Command::ToggleTaskList.default_combo_on(true), "ctrl-t");
        assert_eq!(
            Command::ToggleTaskList.default_combo_on(false),
            "mod-shift-t"
        );
        for command in [
            Command::ShowShortcuts,
            Command::FindInChat,
            Command::OpenModelPicker,
            Command::StopBackgroundTasks,
        ] {
            assert_eq!(
                command.default_combo_on(true),
                command.default_combo_on(false)
            );
        }
        assert_eq!(Command::ShowShortcuts.default_combo(), "mod-/");
        assert_eq!(Command::OpenModelPicker.default_combo(), "alt-p");
    }
    #[test]
    fn invalid_command_and_file_versions_are_rejected() {
        assert!(import(&serde_json::json!({"schemaVersion":2,"rules":[]})).is_err());
        assert!(import(&serde_json::json!({"schemaVersion":1,"rules":[{"key":"mod+n","command":"unknown"}]})).is_err());
        assert!(normalize_key("mod+alt").is_none());
        assert!(normalize_key("mod++a").is_none());
    }
}
