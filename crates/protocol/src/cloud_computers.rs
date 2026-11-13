//! Provider-neutral cloud computers, preparation and versioned environments.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Boat,
    Boxd,
    E2b,
    Daytona,
    Prized,
}
impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Boat,
        Self::Boxd,
        Self::E2b,
        Self::Daytona,
        Self::Prized,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boat => "boat",
            Self::Boxd => "boxd",
            Self::E2b => "e2b",
            Self::Daytona => "daytona",
            Self::Prized => "prized",
        }
    }
    /// The worker-only environment variable holding this provider's API key.
    /// Every list of reserved or scrubbed provider credentials derives from it.
    pub fn api_key_env(self) -> &'static str {
        match self {
            Self::Boat => "BOAT_API_KEY",
            Self::Boxd => "BOXD_API_KEY",
            Self::E2b => "E2B_API_KEY",
            Self::Daytona => "DAYTONA_API_KEY",
            Self::Prized => "PRIZED_API_KEY",
        }
    }
    pub fn is_api_key_env(name: &str) -> bool {
        Self::ALL.iter().any(|id| id.api_key_env() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    Claude,
    Codex,
    Grok,
}
impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerSize {
    pub id: String,
    pub label: String,
    pub vcpus: u32,
    pub memory_mib: u32,
    pub disk_gib: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub id: ProviderId,
    pub configured: bool,
    pub managed_base_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_admission_version: Option<u32>,
    pub preserves_disk: bool,
    pub preserves_memory: bool,
    pub supports_snapshots: bool,
    pub supports_docker: bool,
    pub sizes: Vec<ComputerSize>,
    pub unavailable_reason: Option<String>,
}
impl ProviderCapabilities {
    pub fn can_create(&self) -> bool {
        self.configured && self.managed_base_ready && self.preserves_disk && !self.sizes.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreparationStage {
    Queued,
    Allocating,
    Enrolling,
    Repository,
    Environment,
    Credentials,
    Agent,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreparationStepState {
    Running,
    Completed,
    Failed,
}

/// Guest preparation facts, bound to its admitted allocation and runtime credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePreparationUpdate {
    pub operation_id: String,
    pub stage: PreparationStage,
    pub state: PreparationStepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<PreparationLog>,
}

/// Private setup output. Debug deliberately reports sizes rather than content.
pub const MAX_PREPARATION_LOG_BYTES: usize = 16 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparationLog {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub truncated: bool,
}
impl std::fmt::Debug for PreparationLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparationLog")
            .field("stdout_bytes", &self.stdout.len())
            .field("stderr_bytes", &self.stderr.len())
            .field("success", &self.success)
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComputerAction {
    Sleep,
    Resume,
    RestartRuntime,
    Recreate,
    Delete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreparationProgress {
    pub operation_id: String,
    pub stage: PreparationStage,
    pub error_code: Option<String>,
    pub can_retry: bool,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentSource {
    Project,
    Repository,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupCommands {
    #[serde(default)]
    pub initialize: Option<String>,
    #[serde(default)]
    pub refresh: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartupService {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_service_timeout")]
    pub timeout_seconds: u32,
}
fn default_service_timeout() -> u32 {
    60
}
fn default_setup_timeout() -> u32 {
    900
}
fn version_one() -> u32 {
    1
}

/// Secret references are allowed; secret values are never recipe inputs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentRecipe {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default)]
    pub scripts: SetupCommands,
    #[serde(default)]
    pub startup: Vec<StartupService>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_bindings: Vec<String>,
    #[serde(default = "default_setup_timeout")]
    pub setup_timeout_seconds: u32,
}
impl Default for EnvironmentRecipe {
    fn default() -> Self {
        Self {
            version: 1,
            scripts: SetupCommands::default(),
            startup: vec![],
            environment: BTreeMap::new(),
            secret_bindings: vec![],
            setup_timeout_seconds: default_setup_timeout(),
        }
    }
}
impl EnvironmentRecipe {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1
            || !(1..=3600).contains(&self.setup_timeout_seconds)
            || self.startup.len() > 16
            || self.environment.len() > 128
            || self.secret_bindings.len() > 64
        {
            return Err("invalid_environment_recipe");
        }
        let valid_command = |s: &str| !s.trim().is_empty() && s.len() <= 65536 && !s.contains('\0');
        if self
            .scripts
            .initialize
            .iter()
            .chain(self.scripts.refresh.iter())
            .any(|s| !valid_command(s))
        {
            return Err("invalid_environment_command");
        }
        let mut names = std::collections::BTreeSet::new();
        for service in &self.startup {
            if service.name.is_empty()
                || service.name.len() > 80
                || !service
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                || !names.insert(&service.name)
                || !valid_command(&service.command)
                || !(1..=900).contains(&service.timeout_seconds)
                || service.port == Some(0)
            {
                return Err("invalid_startup_service");
            }
        }
        for (key, value) in &self.environment {
            if key.is_empty()
                || key.len() > 128
                || !key.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_uppercase() || (i > 0 && c.is_ascii_digit())
                })
                || value.len() > 8192
                || value.contains('\0')
                || [
                    "TOKEN",
                    "SECRET",
                    "PASSWORD",
                    "PRIVATE",
                    "CREDENTIAL",
                    "API_KEY",
                ]
                .iter()
                .any(|s| key.contains(s))
            {
                return Err("environment_secret_binding_required");
            }
        }
        if self.secret_bindings.iter().any(|s| {
            s.is_empty()
                || s.len() > 200
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        }) {
            return Err("invalid_secret_binding");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentVersion {
    pub project_id: String,
    pub revision: i64,
    pub source: EnvironmentSource,
    pub recipe: EnvironmentRecipe,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveEnvironment {
    pub expected_revision: i64,
    pub recipe: EnvironmentRecipe,
}

/// One account's machine: what every cloud computer for this account has
/// before any repository is checked out. Built into the account's image, so
/// nothing here may be a secret; secrets stay in the secret store.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineRecipe {
    #[serde(default = "version_one")]
    pub version: u32,
    /// Runs once per image build as `zuse` with sudo: apt packages, tool
    /// installs (bun, pnpm, uv…), global packages, warming package stores.
    #[serde(default)]
    pub setup_script: String,
    /// Skills copied into every agent's skills directory.
    #[serde(default)]
    pub skills: Vec<MachineSkill>,
    /// User-level instructions given to every agent (AGENTS.md / CLAUDE.md).
    #[serde(default)]
    pub instructions: String,
    /// Small text files placed under the home directory (.gitconfig, .npmrc…).
    #[serde(default)]
    pub files: Vec<MachineFile>,
    #[serde(default = "default_machine_timeout")]
    pub setup_timeout_seconds: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineSkill {
    pub name: String,
    /// The skill's SKILL.md.
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineFile {
    /// Relative to the home directory, e.g. `.gitconfig`.
    pub path: String,
    pub content: String,
}

fn default_machine_timeout() -> u32 {
    1800
}

impl Default for MachineRecipe {
    fn default() -> Self {
        Self {
            version: 1,
            setup_script: String::new(),
            skills: vec![],
            instructions: String::new(),
            files: vec![],
            setup_timeout_seconds: default_machine_timeout(),
        }
    }
}

/// Home paths a recipe may never write: credentials, runtime state and the
/// agents' own login stores.
const MACHINE_PROTECTED: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".zuse-data",
    ".zuse",
    ".config/zuse",
    ".codex/auth.json",
    ".claude/.credentials.json",
    ".claude.json",
    ".aws",
    ".docker",
    ".kube",
    ".netrc",
    ".git-credentials",
    ".npmrc.token",
];
const MACHINE_TEXT_LIMIT: usize = 64 * 1024;

impl MachineRecipe {
    /// True when the recipe adds nothing; such accounts use the shared base.
    pub fn is_empty(&self) -> bool {
        self.setup_script.trim().is_empty()
            && self.skills.is_empty()
            && self.instructions.trim().is_empty()
            && self.files.is_empty()
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1 || !(60..=3600).contains(&self.setup_timeout_seconds) {
            return Err("invalid_machine_recipe");
        }
        if self.setup_script.len() > MACHINE_TEXT_LIMIT
            || self.instructions.len() > MACHINE_TEXT_LIMIT
        {
            return Err("machine_recipe_text_too_large");
        }
        if self.skills.len() > 64 || self.files.len() > 32 {
            return Err("machine_recipe_too_many_entries");
        }
        let mut names = std::collections::BTreeSet::new();
        for skill in &self.skills {
            let valid = !skill.name.is_empty()
                && skill.name.len() <= 64
                && skill.name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                })
                && !skill.name.starts_with('-');
            if !valid
                || skill.content.len() > MACHINE_TEXT_LIMIT
                || !names.insert(skill.name.as_str())
            {
                return Err("invalid_machine_skill");
            }
        }
        let mut paths = std::collections::BTreeSet::new();
        for file in &self.files {
            let path = file.path.trim_start_matches("./");
            let segments: Vec<&str> = path.split('/').collect();
            let valid = !path.is_empty()
                && path.len() <= 200
                && !path.starts_with('/')
                && segments
                    .iter()
                    .all(|s| !s.is_empty() && *s != "." && *s != "..")
                && path.bytes().all(|b| b.is_ascii_graphic())
                && !MACHINE_PROTECTED
                    .iter()
                    .any(|p| path == *p || path.starts_with(&format!("{p}/")));
            if !valid || file.content.len() > MACHINE_TEXT_LIMIT || !paths.insert(path.to_owned()) {
                return Err("invalid_machine_file");
            }
        }
        let total = self.setup_script.len()
            + self.instructions.len()
            + self.skills.iter().map(|s| s.content.len()).sum::<usize>()
            + self.files.iter().map(|f| f.content.len()).sum::<usize>();
        if total > 1024 * 1024 {
            return Err("machine_recipe_too_large");
        }
        Ok(())
    }
}

/// The account's current recipe and the state of its image.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineVersion {
    pub revision: i64,
    pub recipe: MachineRecipe,
    pub created_at: i64,
    /// `none` (uses the shared base), `queued`, `building`, `ready`, `failed`.
    pub image_state: String,
    #[serde(default)]
    pub image_error: Option<String>,
    /// The newest build of the current setup script, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<MachineImageBuild>,
    /// The image new computers start from now, while a newer build is
    /// waiting, running or has failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_good_image: Option<MachineImageBuild>,
}

/// One user image build as the owner sees it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MachineImageBuild {
    /// `queued`, `building`, `ready` or `failed`.
    pub state: String,
    /// While building: `allocate`, `setup`, `sanitize`, `capture`, `verify`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Why it was built: `recipe`, `base`, `manual`, `refresh`, `use`.
    #[serde(default)]
    pub reason: String,
    /// The machine recipe revision it was built from.
    #[serde(default)]
    pub recipe_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_release_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Redacted tail of the setup script's output, for a failed build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_excerpt: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMachine {
    pub expected_revision: i64,
    pub recipe: MachineRecipe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingCandidate {
    pub provider_id: ProviderId,
    pub release_id: String,
    pub status: String,
    pub expires_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_candidate: Option<StagingCandidate>,
    pub providers: Vec<ProviderCapabilities>,
    pub can_configure: bool,
    pub can_allocate: bool,
    pub allocation_blockers: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_capabilities_do_not_confuse_credentials_with_a_usable_base() {
        for id in ProviderId::ALL {
            let mut c = ProviderCapabilities {
                id,
                configured: true,
                managed_base_ready: false,
                command_admission_version: None,
                preserves_disk: true,
                preserves_memory: false,
                supports_snapshots: false,
                supports_docker: false,
                sizes: vec![ComputerSize {
                    id: "small".into(),
                    label: "Small".into(),
                    vcpus: 1,
                    memory_mib: 4096,
                    disk_gib: 20,
                }],
                unavailable_reason: None,
            };
            assert!(!c.can_create());
            c.managed_base_ready = true;
            assert!(c.can_create());
            c.preserves_disk = false;
            assert!(!c.can_create());
        }
    }
    #[test]
    fn recipe_supports_setup_and_services_but_not_plaintext_credential_fields() {
        let mut recipe: EnvironmentRecipe = serde_json::from_value(serde_json::json!({"scripts":{"initialize":"sudo apt-get install -y jq\nnpm ci"},"startup":[{"name":"web","command":"npm run dev","port":3000}],"environment":{"NODE_ENV":"development"},"secretBindings":["secret_registry"]})).unwrap();
        assert_eq!(recipe.validate(), Ok(()));
        recipe
            .environment
            .insert("NPM_TOKEN".into(), "must-not-be-in-a-recipe".into());
        assert_eq!(
            recipe.validate(),
            Err("environment_secret_binding_required")
        );
    }
    #[test]
    fn services_need_unique_names_and_finite_readiness_deadlines() {
        let mut recipe = EnvironmentRecipe::default();
        let service = StartupService {
            name: "web".into(),
            command: "npm start".into(),
            port: Some(3000),
            timeout_seconds: 60,
        };
        recipe.startup = vec![service.clone(), service];
        assert_eq!(recipe.validate(), Err("invalid_startup_service"));
        recipe.startup.pop();
        recipe.startup[0].timeout_seconds = 0;
        assert_eq!(recipe.validate(), Err("invalid_startup_service"));
    }
}

#[cfg(test)]
mod machine_recipe_tests {
    use super::*;

    #[test]
    fn machine_recipe_rejects_credential_paths_and_bad_names() {
        let mut recipe = MachineRecipe {
            setup_script: "sudo apt-get install -y ffmpeg".into(),
            skills: vec![MachineSkill {
                name: "release-notes".into(),
                content: "# Release notes".into(),
            }],
            files: vec![MachineFile {
                path: ".gitconfig".into(),
                content: "[user]\n".into(),
            }],
            ..Default::default()
        };
        assert_eq!(recipe.validate(), Ok(()));
        assert!(!recipe.is_empty());
        for path in [
            ".ssh/config",
            ".claude/.credentials.json",
            "../etc/passwd",
            "/root/x",
            "a//b",
            ".codex/auth.json",
        ] {
            recipe.files = vec![MachineFile {
                path: path.into(),
                content: String::new(),
            }];
            assert_eq!(recipe.validate(), Err("invalid_machine_file"), "{path}");
        }
        recipe.files = vec![];
        recipe.skills = vec![MachineSkill {
            name: "Bad Name".into(),
            content: String::new(),
        }];
        assert_eq!(recipe.validate(), Err("invalid_machine_skill"));
        assert!(MachineRecipe::default().is_empty());
    }
}
