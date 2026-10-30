//! Host-owned CI watches. Repair permission is separate from observation.
use crate::change_request_assessment::CiState;
use serde::{Deserialize, Serialize};

pub const MAX_REPAIR_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrWatchRequest {
    pub chat_id: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPrWatchRequest {
    #[serde(flatten)]
    pub watch: PrWatchRequest,
    pub enabled: bool,
    pub auto_fix: bool,
    /// Deliberate fresh budget after reviewing the previous repairs.
    #[serde(default)]
    pub reset_attempts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrWatchStatus {
    pub chat_id: String,
    pub url: String,
    pub branch: String,
    pub enabled: bool,
    pub auto_fix: bool,
    pub attempts: usize,
    pub max_attempts: usize,
    pub ci: CiState,
    /// Monotonic per watch; banners consume transitions even when muted.
    pub failure_sequence: u64,
    pub error: Option<String>,
    pub repairing: bool,
    pub can_auto_fix: bool,
}

/// Initial snapshots seed silently; later failures are consumed regardless of
/// output preferences, so muted notifications never replay when unmuted.
#[derive(Default)]
pub struct PrWatchNotificationState {
    seen: std::collections::HashMap<String, u64>,
    seeded: bool,
}

impl PrWatchNotificationState {
    pub fn consume(&mut self, statuses: Vec<PrWatchStatus>) -> Vec<PrWatchStatus> {
        let seeded = self.seeded;
        self.seeded = true;
        statuses
            .into_iter()
            .filter(|status| {
                let key = format!("{}\n{}", status.chat_id, status.url);
                let previous = self.seen.insert(key, status.failure_sequence).unwrap_or(0);
                seeded && status.failure_sequence > previous
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pr_watch_permission_request_is_flat_and_does_not_reset_budget_implicitly() {
        let request: SetPrWatchRequest = serde_json::from_value(serde_json::json!({
            "chatId":"chat", "url":"https://github.com/a/b/pull/1", "enabled":true, "autoFix":true
        }))
        .unwrap();
        assert_eq!(request.watch.chat_id, "chat");
        assert!(request.auto_fix);
        assert!(!request.reset_attempts);
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["chatId"], "chat");
        assert!(value.get("watch").is_none());
    }
    fn status(sequence: u64) -> PrWatchStatus {
        PrWatchStatus {
            chat_id: "chat".into(),
            url: "https://github.com/a/b/pull/1".into(),
            branch: "topic".into(),
            enabled: true,
            auto_fix: false,
            attempts: 0,
            max_attempts: 3,
            ci: CiState::Failed,
            failure_sequence: sequence,
            error: None,
            repairing: false,
            can_auto_fix: true,
        }
    }
    #[test]
    fn pr_watch_notifications_seed_silently_then_consume_each_failure_once() {
        let mut watch = PrWatchNotificationState::default();
        assert!(watch.consume(vec![status(1)]).is_empty());
        assert!(watch.consume(vec![status(1)]).is_empty());
        assert_eq!(watch.consume(vec![status(2)]).len(), 1);
        // A muted delivery still consumes the sequence and cannot replay.
        assert!(watch.consume(vec![status(2)]).is_empty());
        assert_eq!(watch.consume(vec![status(3)]).len(), 1);
    }
    #[test]
    fn pr_watch_notifications_detect_new_watches_without_replaying_reconnects() {
        let mut watch = PrWatchNotificationState::default();
        watch.consume(vec![]);
        assert_eq!(watch.consume(vec![status(1)]).len(), 1);
        assert!(watch.consume(vec![status(1)]).is_empty());
    }
}
