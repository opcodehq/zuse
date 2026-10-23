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
