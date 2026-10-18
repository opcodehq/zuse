//! Account connection wire contract. Bootstrap secrets are transient transport data.
use serde::Deserialize;
pub const PROTOCOL: &str = "zuse-rpc-json-v1";
pub const WIRE_VERSION: u32 = 1;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bootstrap {
    pub environment_id: String,
    pub origin: String,
    pub connect_token: String,
    pub expires_at: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Authenticate {
    pub protocol: String,
    pub connect_token: String,
}
/// Renew with slack, also accommodating short-lived grants in local integration.
pub fn renew_in_ms(expires_at: i64, now: i64) -> Option<u64> {
    let remaining = expires_at.checked_sub(now)?;
    if remaining <= 0 {
        return None;
    }
    let margin = (remaining / 3).clamp(1, 30000);
    Some((remaining - margin).max(1) as u64)
}

/// Legacy wire metadata alone never opts a host into this account transport.
pub fn supports_record(record: &serde_json::Value) -> bool {
    record["wireProtocolVersion"].as_u64() == Some(WIRE_VERSION as u64)
        && record["capabilities"]["features"]
            .as_array()
            .is_some_and(|features| features.iter().any(|v| v.as_str() == Some(PROTOCOL)))
}

/// One row per environment. Fresh discovery wins over local remembered metadata.
pub fn computer_rows<'a>(
    live: &'a [serde_json::Value],
    saved: &'a [serde_json::Value],
) -> Vec<&'a serde_json::Value> {
    let ids: std::collections::HashSet<&str> = live
        .iter()
        .filter_map(|v| v["environmentId"].as_str())
        .collect();
    live.iter()
        .chain(saved.iter().filter(|value| {
            value["environmentId"]
                .as_str()
                .is_some_and(|id| !ids.contains(id))
        }))
        .collect()
}

/// Stable private-bootstrap rejection marker; frontends provide the user-facing text.
pub const REJECTED: &str = "account_connection.rejected";
