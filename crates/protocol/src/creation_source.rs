//! Source selection prepares a local checkout, Cloud ref or issue context before a chat exists.
use serde::{Deserialize, Serialize};

pub const LIST: &str = "ListCreationSources";
pub const RESOLVE: &str = "ResolveCreationSource";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tab {
    #[default]
    #[serde(rename = "prs")]
    PullRequests,
    #[serde(rename = "branches")]
    Branches,
    #[serde(rename = "issues")]
    Issues,
}
impl Tab {
    pub const ALL: [Self; 3] = [Self::PullRequests, Self::Branches, Self::Issues];
    pub fn label(self) -> &'static str {
        match self {
            Self::PullRequests => "PRs",
            Self::Branches => "Branches",
            Self::Issues => "Issues",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Source {
    Branch { reference: String },
    Pr { number: u64 },
    Issue { number: u64 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub source: Source,
    pub title: String,
    pub lead: String,
    pub branch: Option<String>,
    pub author: Option<String>,
    pub in_use: bool,
}
impl Row {
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [&self.title, &self.lead]
                .into_iter()
                .chain(self.branch.iter())
                .chain(self.author.iter())
                .any(|text| text.to_lowercase().contains(&query))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListRequest {
    pub repo_path: String,
    pub tab: Tab,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveRequest {
    pub repo_path: String,
    pub space_id: Option<String>,
    pub source: Source,
    /// Stable across a retry whose receipt was lost.
    pub request_id: String,
    /// Resolve a sandbox ref without preparing a local checkout.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cloud: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Resolution {
    Checkout {
        label: String,
        outcome: crate::CreateWorktreeOutcome,
    },
    CloudRef {
        label: String,
        base_ref: String,
        branch: Option<String>,
    },
    Issue {
        number: u64,
        title: String,
        attachment_path: String,
        markdown: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upload_id: Option<String>,
    },
}

/// The existing worktree endpoint accepts this only for an explicit source choice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutSource {
    pub source: Source,
    pub request_id: String,
}
