//! Host-owned project creation. No credentials or raw process output cross this boundary.
use serde::{Deserialize, Serialize};

pub const GITHUB_STATUS: &str = "ProjectGithubStatus";
pub const GITHUB_REPOS: &str = "ProjectGithubRepositories";
pub const DEFAULT_PARENT: &str = "ProjectDefaultParent";
pub const START: &str = "StartProjectCreation";
pub const LIST: &str = "ListProjectCreations";
pub const WATCH: &str = "WatchProjectCreations";
pub const CANCEL: &str = "CancelProjectCreation";
pub const RETRY: &str = "RetryProjectCreation";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectTemplate {
    #[default]
    Empty,
    Nextjs,
    Turborepo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectSource {
    Clone {
        url: String,
    },
    Create {
        template: ProjectTemplate,
        #[serde(default)]
        publish: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartProjectCreation {
    /// Stable client-minted id: repeating a submission returns the same operation.
    pub id: String,
    pub name: String,
    pub parent: String,
    pub source: ProjectSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectCreationStatus {
    Running,
    Ready,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreation {
    pub id: String,
    pub device_id: String,
    pub destination: String,
    pub request: StartProjectCreation,
    pub stage: String,
    pub status: ProjectCreationStatus,
    pub percent: Option<u8>,
    #[serde(default)]
    pub files_ready: bool,
    pub error: Option<String>,
    pub warning: Option<String>,
    /// Publishing failure does not make a completed local project unusable.
    pub publish_error: Option<String>,
}

impl ProjectCreation {
    pub fn blocks_execution(&self) -> bool {
        !self.files_ready
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectOperationId {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubProjectStatus {
    pub installed: bool,
    pub authenticated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubRepositoriesRequest {
    #[serde(default = "first_page")]
    pub page: u32,
}
fn first_page() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubProjectRepository {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub ssh_url: String,
    pub https_url: String,
    pub private: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubProjectRepositories {
    pub repositories: Vec<GithubProjectRepository>,
    pub next_page: Option<u32>,
}
