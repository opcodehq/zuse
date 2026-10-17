//! Browser-local draft records. These never authorize or replay an engine action.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const MAX_JOURNAL_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DRAFTS: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDraft {
    pub text: String,
    pub attachments: Vec<Value>,
    pub key: String,
    pub message_id: Option<String>,
    pub chat_id: Option<String>,
    #[serde(default)]
    pub host_id: Option<String>,
    pub not_sent: bool,
}
impl BrowserDraft {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.attachments.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftJournal {
    pub version: u32,
    pub drafts: Vec<BrowserDraft>,
    pub recovery: Vec<BrowserDraft>,
}
impl Default for DraftJournal {
    fn default() -> Self {
        Self {
            version: 1,
            drafts: vec![],
            recovery: vec![],
        }
    }
}
impl DraftJournal {
    pub fn decode(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_JOURNAL_BYTES {
            return Err("Browser draft journal is too large");
        }
        let journal: Self =
            serde_json::from_str(text).map_err(|_| "Browser draft journal is unreadable")?;
        journal.validate()?;
        Ok(journal)
    }
    pub fn encode(&self) -> Result<String, &'static str> {
        self.validate()?;
        let text =
            serde_json::to_string(self).map_err(|_| "Browser drafts could not be encoded")?;
        if text.len() > MAX_JOURNAL_BYTES {
            return Err("Browser draft journal is too large");
        }
        Ok(text)
    }
    fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1 {
            return Err("Browser draft journal has an unsupported version");
        }
        if self.drafts.len().saturating_add(self.recovery.len()) > MAX_DRAFTS {
            return Err("Too many browser drafts to save");
        }
        let mut keys = HashSet::new();
        for draft in &self.drafts {
            if !keys.insert(&draft.key) {
                return Err("Browser draft journal has conflicting draft keys");
            }
        }
        for draft in self.drafts.iter().chain(&self.recovery) {
            if draft.key.len() > 16 * 1024 || draft.attachments.len() > 64 {
                return Err("Browser draft metadata is too large");
            }
            if draft.attachments.iter().any(|attachment| {
                attachment["path"]
                    .as_str()
                    .is_none_or(|path| path.is_empty() || path.len() > 16 * 1024)
            }) {
                return Err("Browser draft attachment reference is invalid");
            }
        }
        Ok(())
    }
    pub fn keep_recovery(&mut self, draft: BrowserDraft) {
        if !draft.is_empty() && !self.recovery.contains(&draft) {
            self.recovery.push(draft);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn preserves_unsent_and_uncertain_delivery_with_attachment_authority() {
        let draft = BrowserDraft {
            text: "Inspect notes".into(),
            key: "chat".into(),
            chat_id: Some("chat".into()),
            host_id: Some("original-host".into()),
            attachments: vec![json!({"path":"/uploads/notes.txt","name":"notes.txt"})],
            not_sent: true,
            ..Default::default()
        };
        let mut pending = draft.clone();
        pending.not_sent = false;
        pending.message_id = Some("pending-message".into());
        let journal = DraftJournal {
            drafts: vec![draft],
            recovery: vec![pending],
            ..Default::default()
        };
        assert_eq!(
            DraftJournal::decode(&journal.encode().unwrap()).unwrap(),
            journal
        );
    }
    #[test]
    fn refuses_corruption_unsupported_versions_conflicting_keys_and_limits() {
        let mut journal = DraftJournal::default();
        assert!(DraftJournal::decode("not json").is_err());
        journal.version = 2;
        assert!(journal.encode().is_err());
        journal.version = 1;
        journal.drafts = vec![BrowserDraft::default(), BrowserDraft::default()];
        assert!(journal.encode().is_err());
        journal.drafts.clear();
        journal.recovery = vec![BrowserDraft::default(); MAX_DRAFTS + 1];
        assert!(journal.encode().is_err());
        journal.recovery.clear();
        journal.drafts.push(BrowserDraft {
            text: "x".repeat(MAX_JOURNAL_BYTES),
            ..Default::default()
        });
        assert!(journal.encode().is_err());
        journal.drafts[0].text.clear();
        journal.drafts[0]
            .attachments
            .push(json!({"data":"not a committed path"}));
        assert!(journal.encode().is_err());
    }
    #[test]
    fn deduplicates_exact_copies_without_losing_distinct_delivery_receipts() {
        let mut journal = DraftJournal::default();
        let draft = BrowserDraft {
            text: "Retain this".into(),
            ..Default::default()
        };
        journal.keep_recovery(draft.clone());
        journal.keep_recovery(draft.clone());
        let mut pending = draft;
        pending.message_id = Some("delivery".into());
        journal.keep_recovery(pending);
        journal.keep_recovery(BrowserDraft::default());
        assert_eq!(journal.recovery.len(), 2);
    }
}
