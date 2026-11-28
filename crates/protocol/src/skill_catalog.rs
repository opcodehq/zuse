//! Curated GitHub packages and checkout installation state.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::WorkspaceTarget;

#[derive(Debug, Deserialize)]
pub struct Repository {
    pub repository: String,
    pub publisher: String,
    pub avatar_id: u64,
    pub revision: String,
    pub stars: u64,
    pub skills: Vec<SkillPackage>,
}

#[derive(Debug, Deserialize)]
pub struct SkillPackage {
    pub id: String,
    pub name: String,
    pub path: String,
    pub title: String,
    pub description: String,
    pub files: Vec<SkillFile>,
}

#[derive(Debug, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub bytes: usize,
    pub sha256: String,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Clone, Copy)]
pub struct CatalogEntry {
    pub source: &'static Repository,
    pub package: &'static SkillPackage,
}

impl CatalogEntry {
    pub fn repository_url(self) -> String {
        format!("https://github.com/{}", self.source.repository)
    }

    pub fn avatar_url(self) -> String {
        format!(
            "https://avatars.githubusercontent.com/u/{}?s=96&v=4",
            self.source.avatar_id
        )
    }

    pub fn install_path(self) -> String {
        format!(".agents/skills/{}", self.package.name)
    }
}

pub fn repositories() -> &'static [Repository] {
    static INDEX: OnceLock<Vec<Repository>> = OnceLock::new();
    INDEX.get_or_init(|| {
        serde_json::from_str(include_str!("skill-index.json")).expect("valid GitHub skill index")
    })
}

pub fn entries() -> impl Iterator<Item = CatalogEntry> {
    repositories().iter().flat_map(|source| {
        source
            .skills
            .iter()
            .map(move |package| CatalogEntry { source, package })
    })
}

pub fn entry(id: &str) -> Option<CatalogEntry> {
    entries().find(|entry| entry.package.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRequest {
    #[serde(flatten)]
    pub target: WorkspaceTarget,
    #[serde(flatten)]
    pub action: CatalogAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_checkout_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum CatalogAction {
    List,
    Install { skill: String },
    Remove { skill: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Installation {
    /// The checkout matches every file in the indexed GitHub package.
    Installed,
    /// The name is already in use. Keep the existing directory intact.
    Existing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogState {
    pub checkout_id: String,
    pub installations: std::collections::BTreeMap<String, Installation>,
}
