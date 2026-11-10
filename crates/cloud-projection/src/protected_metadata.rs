//! Optional small native metadata snapshots in the same store as provisional
//! history keys (see `secure_cache`).
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
const SERVICE: &str = "sh.zuse.cloud-sidebar.catalog.v1";
const LIMIT: usize = 65536;
const MAX_AGE: u64 = 7 * 86400;
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}
fn encode(value: &Value, at: u64) -> Option<Vec<u8>> {
    let bytes = serde_json::to_vec(&json!({"version":1,"savedAt":at,"value":value})).ok()?;
    (bytes.len() <= LIMIT).then_some(bytes)
}
fn decode(bytes: &[u8], at: u64) -> Option<Value> {
    if bytes.len() > LIMIT {
        return None;
    }
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let saved = value["savedAt"].as_u64()?;
    (value["version"] == 1 && at >= saved && at - saved <= MAX_AGE).then(|| value["value"].clone())
}
pub fn read(namespace: &str) -> Option<Value> {
    let bytes = zeroize::Zeroizing::new(crate::secure_cache::read(SERVICE, namespace)?);
    decode(&bytes, now())
}
pub fn write(namespace: &str, value: &Value) {
    if let Some(bytes) = encode(value, now()) {
        let bytes = zeroize::Zeroizing::new(bytes);
        crate::secure_cache::write(SERVICE, namespace, &bytes);
    }
}
pub fn clear(namespace: Option<&str>) {
    crate::secure_cache::clear(SERVICE, namespace);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_metadata_roundtrips_and_old_future_corrupt_or_large_records_miss() {
        let value = json!({"projects":[{"projectId":"p"}]});
        let bytes = encode(&value, 100).unwrap();
        assert_eq!(decode(&bytes, 101), Some(value));
        assert!(decode(&bytes, 99).is_none());
        assert!(decode(&bytes, 100 + MAX_AGE + 1).is_none());
        assert!(decode(b"not-json", 101).is_none());
        assert!(encode(&json!({"text":"x".repeat(LIMIT)}), 100).is_none());
    }
}
