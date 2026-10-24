//! Recorded usage is separate from context occupancy and live plan allowances.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const OVERVIEW: &str = "UsageOverview";
pub const SESSIONS: &str = "UsageSessions";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
}
impl TokenCounts {
    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.reasoning_tokens)
    }
    pub fn add(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostStatus {
    Known,
    Partial,
    #[default]
    Unknown,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    #[serde(flatten)]
    pub tokens: TokenCounts,
    pub cost_usd: Option<f64>,
    pub cost_status: CostStatus,
    pub record_count: usize,
    pub possible_duplicate_count: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub key: String,
    pub label: String,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub source_ids: Vec<String>,
    #[serde(flatten)]
    pub summary: Summary,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub id: String,
    pub label: String,
    pub detected: bool,
    pub record_count: usize,
    pub paths: Vec<String>,
    pub warning: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub bucket: String,
    pub generated_at: DateTime<Utc>,
    pub summary: Summary,
    pub session_count: usize,
    pub previous_summary: Option<Summary>,
    pub previous_session_count: Option<usize>,
    pub groups: Vec<Group>,
    pub by_source: Vec<Group>,
    pub by_model: Vec<Group>,
    pub by_project: Vec<Group>,
    pub previous_by_source: Vec<Group>,
    pub previous_by_model: Vec<Group>,
    pub previous_by_project: Vec<Group>,
    pub sources: Vec<SourceStatus>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsPage {
    pub rows: Vec<Group>,
    pub total: usize,
    pub next_offset: Option<usize>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Request {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub project_id: Option<String>,
    pub timezone: Option<String>,
    pub force_refresh: bool,
    pub query: String,
    pub provider_id: Option<String>,
    pub sort: SessionSort,
    pub offset: usize,
    pub limit: Option<usize>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionSort {
    #[default]
    Tokens,
    Cost,
    LastActive,
}
