//! Client presentation state shared without filesystem or rendering dependencies.
mod cloud_projects;
mod effort;
pub use cloud_projects::{CloudProjectGroup, cloud_project_group};
pub mod git_history;
mod model_config;
pub mod motion;
pub use model_config::RememberedModelConfig;
mod queue;
pub use effort::{EffortSetting, EffortStop, ModelEffort, model_effort, model_effort_option};
pub use queue::{
    QueueAction, QueueEditLease, QueuePrimaryAction, QueueRowDetails, chat_request,
    queue_edit_lease, queue_edit_review_matches, queue_primary_action, queue_row_details,
};

use crate::Model;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidebarView {
    #[default]
    Threads,
    Projects,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppearanceMode {
    #[default]
    System,
    Light,
    Dark,
}
impl AppearanceMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Permission postures a new conversation can start with. The order is the
/// order the settings select lists them in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    #[default]
    ApprovalRequired,
    /// "Approve for me": the agent's automatic approver decides.
    Auto,
    FullAccess,
}
impl PermissionMode {
    pub const ALL: [Self; 3] = [Self::ApprovalRequired, Self::Auto, Self::FullAccess];
    pub fn label(self) -> &'static str {
        access_details(self.sandbox()).label
    }
    pub fn sandbox(self) -> crate::SandboxLevel {
        match self {
            Self::ApprovalRequired => crate::SandboxLevel::WorkspaceWrite,
            Self::Auto => crate::SandboxLevel::Auto,
            Self::FullAccess => crate::SandboxLevel::DangerFullAccess,
        }
    }
    /// Defaults apply only before a conversation exists. An existing chat
    /// without stored configuration keeps the conservative legacy posture.
    pub fn for_chat(self, chat: Option<&crate::Chat>) -> crate::SandboxLevel {
        match chat {
            Some(chat) => chat
                .config
                .as_ref()
                .map(|config| config.sandbox)
                .unwrap_or_default(),
            None => self.sandbox(),
        }
    }
}

/// Shared composer labels and Hugeicons identities for actual run postures.
pub struct AccessDetails {
    pub label: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
}
/// The composer's access menu order. Plan mode is a separate toggle.
pub const ACCESS_LEVELS: [crate::SandboxLevel; 3] = [
    crate::SandboxLevel::WorkspaceWrite,
    crate::SandboxLevel::Auto,
    crate::SandboxLevel::DangerFullAccess,
];
/// Descriptions stay one short line so the menu never wraps.
pub fn access_details(level: crate::SandboxLevel) -> AccessDetails {
    match level {
        crate::SandboxLevel::WorkspaceWrite => AccessDetails {
            label: "Ask for approval",
            description: "Asks before restricted actions.",
            icon: "icons/access-ask.svg",
        },
        crate::SandboxLevel::Auto => AccessDetails {
            label: "Approve for me",
            description: "Reviews actions, asks when needed.",
            icon: "icons/access-auto.svg",
        },
        crate::SandboxLevel::DangerFullAccess => AccessDetails {
            label: "Full access",
            description: "Runs actions without approval.",
            icon: "icons/access-full.svg",
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultModelSelection {
    pub provider: crate::HarnessId,
    pub model: String,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceMode {
    #[default]
    Worktree,
    Local,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspacePreferences {
    pub display_name: String,
    pub sidebar_width: Option<u16>,
    pub sidebar_view: SidebarView,
    pub appearance: AppearanceMode,
    pub reduce_motion: motion::ReduceMotion,
    pub pause_animations_in_background: bool,
    pub completion_sound: crate::completion_sound::CompletionSoundPreferences,
    pub provider_update_notifications: bool,
    pub default_permission_mode: PermissionMode,
    pub collapsed_projects: Vec<String>,
    pub workspace_by_project: BTreeMap<String, WorkspaceMode>,
    pub default_provider: Option<String>,
    pub cloud_provider: Option<String>,
    pub cloud_sizes: BTreeMap<String, String>,
    pub default_models: BTreeMap<String, String>,
    pub recent_models: Vec<ModelSelection>,
    pub model_config_history: Vec<RememberedModelConfig>,
}
impl WorkspacePreferences {
    pub fn starting_model(&self) -> Option<(String, Option<String>)> {
        if let Some(provider) = &self.default_provider {
            return Some((provider.clone(), self.default_models.get(provider).cloned()));
        }
        if let Some(config) = self.model_config_history.first() {
            return Some((config.provider.clone(), config.model.clone()));
        }
        self.recent_models
            .first()
            .map(|choice| (choice.provider.clone(), Some(choice.model.clone())))
    }

    pub fn model_config(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> Option<&RememberedModelConfig> {
        self.model_config_history
            .iter()
            .find(|entry| entry.provider == provider && entry.model.as_deref() == model)
    }
    pub fn remember_model_config(&mut self, config: RememberedModelConfig) {
        self.model_config_history
            .retain(|entry| entry.provider != config.provider || entry.model != config.model);
        self.model_config_history.insert(0, config);
        model_config::normalize_history(&mut self.model_config_history);
    }

    pub fn record_model(&mut self, selection: ModelSelection) {
        self.recent_models.retain(|old| old != &selection);
        self.recent_models.insert(0, selection);
        self.recent_models.truncate(8);
    }
    pub fn set_default(&mut self, selection: ModelSelection) {
        self.default_provider = Some(selection.provider.clone());
        self.default_models
            .insert(selection.provider.clone(), selection.model.clone());
        self.record_model(selection);
    }
    pub fn normalize(&mut self) {
        self.sidebar_width = self.sidebar_width.map(|width| width.clamp(200, 300));
        let provider = |id: &str| {
            crate::cloud_computers::ProviderId::ALL
                .iter()
                .any(|p| p.as_str() == id)
        };
        if self
            .cloud_provider
            .as_deref()
            .is_some_and(|id| !provider(id))
        {
            self.cloud_provider = None;
        }
        self.cloud_sizes.retain(|id, size| {
            provider(id)
                && !size.is_empty()
                && size.len() <= 64
                && size
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        });
        self.recent_models.truncate(8);
        model_config::normalize_history(&mut self.model_config_history);
        self.collapsed_projects.truncate(512);
        self.default_models
            .retain(|provider, model| provider.len() <= 128 && model.len() <= 1024);
    }
}

#[derive(Default)]
pub struct ProviderCatalog {
    pub models: Vec<Model>,
    pub error: Option<String>,
    pub request: Option<u64>,
    pub loaded: bool,
    pub stale: bool,
}
#[derive(Default)]
pub struct PickerCatalog {
    pub providers: BTreeMap<String, ProviderCatalog>,
    requests: BTreeMap<u64, String>,
}
impl PickerCatalog {
    pub fn begin(&mut self, provider: &str, request: u64) {
        let catalog = self.providers.entry(provider.to_owned()).or_default();
        if let Some(old) = catalog.request.replace(request) {
            self.requests.remove(&old);
        }
        catalog.error = None;
        self.requests.insert(request, provider.to_owned());
    }
    pub fn complete(&mut self, request: u64, result: Result<Vec<Model>, String>) -> bool {
        let Some(provider) = self.requests.remove(&request) else {
            return false;
        };
        let catalog = self.providers.get_mut(&provider).unwrap();
        if catalog.request != Some(request) {
            return false;
        }
        catalog.request = None;
        catalog.stale = false;
        match result {
            Ok(models) => {
                catalog.models = models;
                catalog.error = None;
                catalog.loaded = true;
            }
            Err(error) => catalog.error = Some(error),
        }
        true
    }
    pub fn contains_request(&self, id: u64) -> bool {
        self.requests.contains_key(&id)
    }
    pub fn retire(&mut self) -> Vec<u64> {
        let ids = self.requests.keys().copied().collect();
        *self = Self::default();
        ids
    }
    pub fn interrupted(&mut self) {
        self.requests.clear();
        for catalog in self.providers.values_mut() {
            catalog.stale = true;
            if catalog.request.take().is_some() {
                catalog.error =
                    Some("Connection interrupted. Refresh to check available models.".into());
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectIdentity {
    pub key: String,
    pub path: String,
    pub name: String,
}
pub fn project_identity(
    cwd: Option<&str>,
    space_id: Option<&str>,
    spaces: &[crate::Space],
    roots: &[&str],
) -> ProjectIdentity {
    if crate::projectless::is_projectless_cwd(cwd) {
        return ProjectIdentity {
            key: String::new(),
            path: String::new(),
            name: "No project".into(),
        };
    }
    if let Some(space) = space_id.and_then(|id| spaces.iter().find(|s| s.id == id)) {
        let representative = crate::view::representative_space(spaces, space);
        return ProjectIdentity {
            key: crate::view::project_key(space),
            path: representative.path.clone(),
            name: representative.display_name().to_owned(),
        };
    }
    let path = project_root(cwd, roots.iter().copied())
        .unwrap_or_default()
        .to_owned();
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(&path)
        .to_owned();
    ProjectIdentity {
        key: format!("folder:{path}"),
        path,
        name,
    }
}

/// A nested folder belongs to the longest matching repository root. Scratch
/// conversations remain projectless; basename collisions never merge projects.
pub fn project_root<'a>(
    cwd: Option<&'a str>,
    roots: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let cwd = cwd?.trim_end_matches('/');
    if crate::projectless::is_projectless_cwd(Some(cwd)) {
        return None;
    }
    roots
        .into_iter()
        .filter(|root| {
            let root = root.trim_end_matches('/');
            cwd == root
                || cwd
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|root| root.len())
        .or(Some(cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model(id: &str) -> Model {
        serde_json::from_value(serde_json::json!({"id":id,"label":id})).unwrap()
    }
    #[test]
    fn provider_replies_and_reconnect_cannot_replace_another_catalog() {
        let mut state = PickerCatalog::default();
        state.begin("claude-code", 1);
        state.begin("codex", 2);
        assert!(state.complete(2, Ok(vec![model("codex-live")])));
        assert!(state.complete(1, Ok(vec![model("claude-live")])));
        state.begin("claude-code", 3);
        state.begin("claude-code", 4);
        assert!(!state.complete(3, Ok(vec![model("stale")])));
        state.interrupted();
        assert!(!state.complete(4, Ok(vec![model("old-link")])));
        assert_eq!(state.providers["claude-code"].models[0].id, "claude-live");
        assert_eq!(state.providers["codex"].models[0].id, "codex-live");
        state.begin("codex", 5);
        assert!(state.complete(5, Err("Sign in required".into())));
        assert_eq!(state.providers["codex"].models[0].id, "codex-live");
        assert_eq!(
            state.providers["codex"].error.as_deref(),
            Some("Sign in required")
        );
    }
    #[test]
    fn nested_roots_and_scratch_chats_keep_their_project_identity() {
        let roots = ["/work/app", "/work/app/nested", "/other/app"];
        assert_eq!(
            project_root(Some("/work/app/nested/src"), roots),
            Some("/work/app/nested")
        );
        assert_eq!(project_root(Some("/work/app2"), roots), Some("/work/app2"));
        assert_eq!(
            project_root(Some("/other/app/src"), roots),
            Some("/other/app")
        );
        assert_eq!(
            project_root(Some("~/.zuse/scratch/2026-10-17-test"), roots),
            None
        );
        assert_eq!(project_root(None, roots), None);
    }
    #[test]
    fn repository_spaces_group_worktrees_with_a_stable_representative() {
        let make = |id: &str, path: &str, created: &str| {
            serde_json::from_value::<crate::Space>(serde_json::json!({"id":id,"deviceId":"local","path":path,"repositoryId":"commit:shared","createdAt":created})).unwrap()
        };
        let spaces = vec![
            make("main", "/work/api", "2026-01-01T00:00:00Z"),
            make("worktree", "/trees/api-fix", "2026-02-01T00:00:00Z"),
        ];
        let main = project_identity(Some("/work/api/src"), Some("main"), &spaces, &[]);
        let tree = project_identity(Some("/trees/api-fix"), Some("worktree"), &spaces, &[]);
        assert_eq!(main, tree);
        assert_eq!(tree.path, "/work/api");
        assert_eq!(
            project_identity(Some("~"), None, &spaces, &[]).name,
            "No project"
        );
    }
    #[test]
    fn defaults_and_recent_choices_keep_provider_identity_and_round_trip() {
        let mut prefs = WorkspacePreferences::default();
        for i in 0..12 {
            prefs.record_model(ModelSelection {
                provider: "codex".into(),
                model: i.to_string(),
            });
        }
        let choice = ModelSelection {
            provider: "claude-code".into(),
            model: "11".into(),
        };
        prefs.set_default(choice.clone());
        prefs.record_model(choice.clone());
        prefs.sidebar_view = SidebarView::Projects;
        let decoded: WorkspacePreferences =
            serde_json::from_value(serde_json::to_value(&prefs).unwrap()).unwrap();
        assert_eq!(decoded.recent_models.len(), 8);
        assert_eq!(decoded.recent_models[0], choice);
        assert_eq!(decoded.default_models["claude-code"], "11");
        assert_eq!(decoded.sidebar_view, SidebarView::Projects);
    }
}

/// Switches represent boolean speed settings and binary context sizes;
/// two arbitrary model names remain a choice menu.
pub fn model_option_switch_values<'a>(
    id: &str,
    choices: impl IntoIterator<Item = &'a str>,
) -> Option<(&'a str, &'a str)> {
    let mut choices = choices.into_iter();
    let values = [choices.next()?, choices.next()?];
    if choices.next().is_some() {
        return None;
    }
    let has = |value: &str| {
        values
            .iter()
            .any(|choice| choice.eq_ignore_ascii_case(value))
    };
    let boolean = (has("on") && has("off")) || (has("true") && has("false"));
    if !boolean && !has("fast") && !id.to_ascii_lowercase().contains("context") {
        return None;
    }
    binary_switch_values(values)
}

/// Canonical enabled/disabled identities for a binary model option. Stored
/// defaults may themselves be enabled; they do not determine switch position.
pub fn binary_switch_values<'a>(
    choices: impl IntoIterator<Item = &'a str>,
) -> Option<(&'a str, &'a str)> {
    let mut choices = choices.into_iter();
    let first = choices.next()?;
    let second = choices.next()?;
    if choices.next().is_some() {
        return None;
    }
    let enabled = |value: &str| {
        ["on", "true", "fast"]
            .iter()
            .any(|on| value.eq_ignore_ascii_case(on))
    };
    let context_size = |value: &str| {
        let value = value.to_ascii_lowercase();
        let (number, scale) = if let Some(value) = value.strip_suffix('m') {
            (value, 1_000_000.)
        } else if let Some(value) = value.strip_suffix('k') {
            (value, 1_000.)
        } else {
            return None;
        };
        number.parse::<f64>().ok().map(|number| number * scale)
    };
    if enabled(first)
        || (!enabled(second)
            && context_size(first)
                .zip(context_size(second))
                .is_some_and(|(first, second)| first > second))
    {
        Some((first, second))
    } else {
        Some((second, first))
    }
}

#[cfg(test)]
mod binary_switch_tests {
    use super::{binary_switch_values, model_option_switch_values};
    #[test]
    fn enabled_identity_survives_reordered_catalogs_and_enabled_defaults() {
        for (choices, expected) in [
            (["off", "on"], ("on", "off")),
            (["true", "false"], ("true", "false")),
            (["fast", "default"], ("fast", "default")),
            (["1m", "200k"], ("1m", "200k")),
            (["200k", "1m"], ("1m", "200k")),
        ] {
            assert_eq!(binary_switch_values(choices), Some(expected));
        }
        assert_eq!(binary_switch_values(["off", "auto", "on"]), None);
        assert_eq!(model_option_switch_values("lead", ["fable", "sol"]), None);
        assert_eq!(
            model_option_switch_values("contextWindow", ["1m", "200k"]),
            Some(("1m", "200k"))
        );
        assert_eq!(
            model_option_switch_values("serviceTier", ["default", "fast"]),
            Some(("fast", "default"))
        );
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use crate::SandboxLevel;
    #[test]
    fn new_defaults_never_rewrite_existing_or_legacy_chat_permissions() {
        let mut chat: crate::Chat = serde_json::from_value(serde_json::json!({"id":"c","deviceId":"d","archived":false,"createdAt":"2026-01-01T00:00:00Z","config":{"harness":"codex","sandbox":"read-only"}})).unwrap();
        assert_eq!(
            PermissionMode::FullAccess.for_chat(Some(&chat)),
            SandboxLevel::WorkspaceWrite
        );
        chat.config.as_mut().unwrap().sandbox = SandboxLevel::Auto;
        assert_eq!(
            PermissionMode::FullAccess.for_chat(Some(&chat)),
            SandboxLevel::Auto
        );
        chat.config = None;
        assert_eq!(
            PermissionMode::FullAccess.for_chat(Some(&chat)),
            SandboxLevel::WorkspaceWrite
        );
        assert_eq!(
            PermissionMode::FullAccess.for_chat(None),
            SandboxLevel::DangerFullAccess
        );
        assert_eq!(
            PermissionMode::ApprovalRequired.for_chat(None),
            SandboxLevel::WorkspaceWrite
        );
        let preferences: WorkspacePreferences =
            serde_json::from_value(serde_json::json!({"sidebarView":"projects"})).unwrap();
        assert_eq!(
            preferences.default_permission_mode,
            PermissionMode::ApprovalRequired
        );
    }

    #[test]
    fn auto_permission_mode_round_trips_and_maps_to_auto_sandbox() {
        let preferences: WorkspacePreferences =
            serde_json::from_value(serde_json::json!({"defaultPermissionMode":"auto"})).unwrap();
        assert_eq!(preferences.default_permission_mode, PermissionMode::Auto);
        assert_eq!(
            serde_json::to_value(PermissionMode::Auto).unwrap(),
            serde_json::json!("auto")
        );
        for mode in ["approval-required", "full-access"] {
            serde_json::from_value::<PermissionMode>(serde_json::json!(mode)).unwrap();
        }
        assert_eq!(PermissionMode::Auto.for_chat(None), SandboxLevel::Auto);
        assert_eq!(PermissionMode::Auto.label(), "Approve for me");
        assert_eq!(
            ACCESS_LEVELS.map(|level| access_details(level).label),
            ["Ask for approval", "Approve for me", "Full access"]
        );
    }
}

/// A readable account label without exposing the full mailbox in the UI.
pub fn masked_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => format!("{}•••@{domain}", local.chars().next().unwrap_or('•')),
        None => "•••".to_owned(),
    }
}
