//! Connection generations fence stale work; reconnect never resends requests.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptedRequest {
    pub id: u64,
    pub method: String,
    pub mutation: bool,
}
#[derive(Default)]
pub struct BrowserLink {
    epoch: u64,
    online: bool,
    pending: BTreeMap<u64, InterruptedRequest>,
}
impl BrowserLink {
    pub fn open(&mut self) -> u64 {
        self.pending.clear();
        self.epoch = self.epoch.saturating_add(1);
        self.online = true;
        self.epoch
    }
    pub fn admit(&mut self, epoch: u64, id: u64, method: &str, mutation: bool) -> bool {
        if !self.online || epoch != self.epoch || self.pending.contains_key(&id) {
            return false;
        }
        self.pending.insert(
            id,
            InterruptedRequest {
                id,
                method: method.into(),
                mutation,
            },
        );
        true
    }
    pub fn cancel(&mut self, epoch: u64, id: u64) -> bool {
        self.online && epoch == self.epoch && self.pending.remove(&id).is_some()
    }
    pub fn receive(&mut self, epoch: u64, id: u64, terminal: bool) -> bool {
        if !self.online || epoch != self.epoch || !self.pending.contains_key(&id) {
            return false;
        }
        if terminal {
            self.pending.remove(&id);
        }
        true
    }
    pub fn close(&mut self) -> Vec<InterruptedRequest> {
        self.online = false;
        std::mem::take(&mut self.pending).into_values().collect()
    }
}
pub fn retry_delay_ms(attempt: u32) -> u32 {
    250u32.saturating_mul(1u32 << attempt.min(5)).min(8_000)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lost_ack_is_reported_once_and_old_generation_work_never_reaches_the_new_link() {
        let mut link = BrowserLink::default();
        let first = link.open();
        assert!(link.admit(first, 1, "QueueCommand", true));
        assert!(link.admit(first, 2, "WatchDocMessages", false));
        assert!(link.receive(first, 2, false));
        let lost = link.close();
        assert_eq!(
            lost.iter().map(|r| (r.id, r.mutation)).collect::<Vec<_>>(),
            [(1, true), (2, false)]
        );
        assert!(link.close().is_empty());
        let second = link.open();
        assert_ne!(first, second);
        assert!(!link.admit(first, 3, "WriteWorkspaceFile", true));
        assert!(!link.receive(first, 1, true));
        assert!(link.admit(second, 4, "WatchDocMessages", false));
        assert!(link.receive(second, 4, false));
        assert!(!link.cancel(first, 4));
        assert!(link.cancel(second, 4));
        assert!(!link.receive(second, 4, false));
        assert!(link.close().is_empty());
    }
    #[test]
    fn acknowledged_work_is_not_ambiguous_and_reconnect_backoff_is_bounded() {
        let mut link = BrowserLink::default();
        let epoch = link.open();
        assert!(link.admit(epoch, 9, "WriteWorkspaceFile", true));
        assert!(link.receive(epoch, 9, true));
        assert!(link.close().is_empty());
        assert!(!link.admit(epoch, 10, "QueueCommand", true));
        assert_eq!(retry_delay_ms(0), 250);
        assert_eq!(retry_delay_ms(u32::MAX), 8_000);
    }
}
