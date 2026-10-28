//! Bounded, observational cache diagnostics. Never retain prompt text or secrets.
use super::Attempt;
use crate::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Write, sync::Mutex};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub tool_total_ms: Option<f64>,
    pub tool_calls: Option<u64>,
    pub storage_total_ms: Option<f64>,
    pub admission_ms: Option<f64>,
    pub preparation_ms: Option<f64>,
    pub headers_ms: Option<f64>,
    pub first_content_ms: Option<f64>,
    pub first_text_ms: Option<f64>,
    pub stream_ms: Option<f64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrefixChange {
    #[default]
    FirstRequest,
    Account,
    Model,
    Configuration,
    History,
    Append,
    Unchanged,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheRead {
    #[default]
    Unknown,
    Observed,
    Zero,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheDiagnostic {
    pub read: CacheRead,
    pub prefix_change: PrefixChange,
}
struct Entry {
    connection: String,
    model: String,
    config: String,
    len: usize,
    hash: [u8; 32],
}
pub(super) struct Probe {
    key: (String, String),
    entry: Entry,
    change: PrefixChange,
}
#[derive(Default)]
pub(super) struct Monitor(Mutex<HashMap<(String, String), Entry>>);
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Monitor {
    pub fn begin(
        &self,
        root: &str,
        agent: &str,
        connection: &str,
        model: &str,
        config: &str,
        messages: &[Message],
    ) -> Probe {
        let key = (root.into(), agent.into());
        let previous = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .map(|p| {
                (
                    p.connection.clone(),
                    p.model.clone(),
                    p.config.clone(),
                    p.len,
                    p.hash,
                )
            });
        let mut hash = HashWriter(Sha256::new());
        let mut prefix = None;
        for (i, message) in messages.iter().enumerate() {
            serde_json::to_writer(&mut hash, message).expect("message hashes serialize");
            hash.0.update([0]);
            if previous.as_ref().is_some_and(|p| p.3 == i + 1) {
                prefix = Some(<[u8; 32]>::from(hash.0.clone().finalize()));
            }
        }
        let hash = hash.0.finalize().into();
        let change = match &previous {
            None => PrefixChange::FirstRequest,
            Some(p) if p.0 != connection => PrefixChange::Account,
            Some(p) if p.1 != model => PrefixChange::Model,
            Some(p) if p.2 != config => PrefixChange::Configuration,
            Some(p) if p.3 > messages.len() || (p.3 > 0 && prefix != Some(p.4)) => {
                PrefixChange::History
            }
            Some(p) if p.3 < messages.len() => PrefixChange::Append,
            _ => PrefixChange::Unchanged,
        };
        Probe {
            key,
            entry: Entry {
                connection: connection.into(),
                model: model.into(),
                config: config.into(),
                len: messages.len(),
                hash,
            },
            change,
        }
    }
    pub fn finish(&self, probe: Probe, attempt: &mut Attempt) {
        attempt.cache = CacheDiagnostic {
            prefix_change: probe.change,
            read: match attempt.usage.as_ref().and_then(|u| u.cached_tokens) {
                Some(0) => CacheRead::Zero,
                Some(_) => CacheRead::Observed,
                None => CacheRead::Unknown,
            },
        };
        if attempt.dispatched && attempt.error_code.is_none() {
            let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if entries.len() >= 4096
                && !entries.contains_key(&probe.key)
                && let Some(key) = entries.keys().next().cloned()
            {
                entries.remove(&key);
            }
            entries.insert(probe.key, probe.entry);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Role, Usage};
    #[test]
    fn cache_changes_preserve_unknowns_and_separate_observed_reads_from_invalidation() {
        let monitor = Monitor::default();
        let mut history = vec![Message::text(Role::User, "same")];
        let observe = |history: &[Message], account: &str, model: &str, config: &str, cached| {
            let probe = monitor.begin("root", "agent", account, model, config, history);
            let mut a = Attempt {
                dispatched: true,
                usage: Some(Usage {
                    cached_tokens: cached,
                    ..Default::default()
                }),
                ..Default::default()
            };
            monitor.finish(probe, &mut a);
            a.cache
        };
        assert_eq!(
            observe(&history, "a", "m", "c", None).read,
            CacheRead::Unknown
        );
        history.push(Message::text(Role::Assistant, "answer"));
        let appended = observe(&history, "a", "m", "c", Some(23));
        assert_eq!(appended.prefix_change, PrefixChange::Append);
        assert_eq!(appended.read, CacheRead::Observed);
        assert_eq!(
            observe(&history, "b", "m", "c", Some(0)).prefix_change,
            PrefixChange::Account
        );
        assert_eq!(
            observe(&history, "b", "n", "c", Some(0)).prefix_change,
            PrefixChange::Model
        );
        assert_eq!(
            observe(&history, "b", "n", "d", Some(0)).prefix_change,
            PrefixChange::Configuration
        );
        history[0] = Message::text(Role::User, "changed");
        assert_eq!(
            observe(&history, "b", "n", "d", Some(0)).prefix_change,
            PrefixChange::History
        );
    }
}
