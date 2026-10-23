//! Existing repository catalog contracts. Environment values stay in the control plane.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConnectRequest {
    pub repository_url: String,
    pub default_branch: String,
    pub visibility: ProjectVisibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub cloud_environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_bindings: Vec<String>,
    pub idempotency_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectVisibility {
    Public,
    Private,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBuildStatus {
    pub build_id: String,
    pub provider_id: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProject {
    pub project_id: String,
    pub repository_identity: String,
    pub repository_url: String,
    pub display_name: String,
    pub default_branch: String,
    pub visibility: ProjectVisibility,
    pub state: String,
    pub active_builds: BTreeMap<String, String>,
    pub latest_builds: BTreeMap<String, ProjectBuildStatus>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectList {
    pub projects: Vec<CloudProject>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRemoveRequest {
    pub project_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepository {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub ssh_url: String,
    pub https_url: String,
    pub is_private: bool,
    pub default_branch: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_avatar_url: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubInstallation {
    pub installation_id: i64,
    pub account_login: String,
    pub account_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    pub repository_selection: String,
    pub suspended: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct GithubUser {
    pub login: String,
    pub name: String,
    pub email: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubCatalog {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_installations: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<GithubUser>,
    pub installations: Vec<GithubInstallation>,
    pub repositories: Vec<GithubRepository>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageBuildMode {
    Update,
    Rebuild,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageBuildRequest {
    pub mode: ImageBuildMode,
    pub provider_id: Option<String>,
    pub idempotency_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudImageRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub display_name: String,
    pub default_branch: String,
    pub source_commit: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudImageBuild {
    pub build_id: String,
    pub state: String,
    pub mode: ImageBuildMode,
    pub active: bool,
    pub progress_phase: Option<String>,
    pub error_code: Option<String>,
    pub log_text: Option<String>,
    pub runtime_version: String,
    pub configuration_digest: String,
    pub repositories: Vec<CloudImageRepository>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccountImage {
    pub state: String,
    pub generation: Option<String>,
    pub provider_id: Option<String>,
    pub runtime_version: Option<String>,
    pub progress_phase: Option<String>,
    pub error_code: Option<String>,
    pub repositories: Vec<CloudImageRepository>,
    pub builds: Vec<CloudImageBuild>,
    pub built_at: Option<i64>,
    pub updated_at: i64,
}
