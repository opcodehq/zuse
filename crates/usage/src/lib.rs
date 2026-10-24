//! Read-only usage imports and report projections shared by the host's dashboard RPCs.
pub mod aggregate;
pub mod import;
pub mod ledger;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use zuse_protocol::usage::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub source_id: String,
    pub source_label: String,
    pub provider_id: String,
    pub model: String,
    pub session_id: Option<String>,
    pub project_path: Option<String>,
    pub workspace_path: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    #[serde(flatten)]
    pub tokens: TokenCounts,
    pub cost_usd: Option<f64>,
    pub provenance: String,
    pub confidence: String,
    pub possible_duplicate: bool,
}
impl Record {
    /// IDs depend on source identity, never file order or the scan's wall clock.
    pub fn identify(&mut self, identity: &str) {
        self.id = format!(
            "{}-{:x}",
            self.source_id,
            Sha256::digest(identity.as_bytes())
        );
        self.cost_usd = self.cost_usd.filter(|cost| cost.is_finite() && *cost >= 0.);
    }
}
#[derive(Debug, Clone, Default)]
pub struct Data {
    pub records: Vec<Record>,
    pub sources: Vec<SourceStatus>,
}
