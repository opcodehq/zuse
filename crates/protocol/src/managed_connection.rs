//! Account connection wire contract. Bootstrap secrets are transient transport data.
use serde::{Deserialize, Serialize};
/// Public connection recipe. Each controller is authorized independently;
/// neither URLs nor this metadata carry a connection capability.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerTarget {
    #[serde(rename = "environmentId")]
    pub environment: String,
    #[serde(rename = "expectedSubject")]
    pub subject: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllers: Vec<ComputerTarget>,
}
impl ComputerTarget {
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let target: Self = serde_json::from_value(value.clone())
            .map_err(|_| "Invalid account computer link".to_owned())?;
        target.validate()?;
        Ok(target)
    }
    fn validate(&self) -> Result<(), String> {
        if self.controllers.len() > 3 {
            return Err("Return to a controller tab before connecting to another computer.".into());
        }
        if self
            .controllers
            .iter()
            .any(|target| !target.controllers.is_empty())
        {
            return Err("Invalid account computer link".into());
        }
        let mut ids = std::collections::HashSet::new();
        for target in self.controllers.iter().chain(std::iter::once(self)) {
            if [&target.environment, &target.subject]
                .iter()
                .any(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
                || target.label.is_empty()
                || target.label.len() > 128
                || target.label.chars().any(char::is_control)
            {
                return Err("Invalid account computer link".into());
            }
            if !ids.insert(target.environment.as_str()) {
                return Err(
                    "That computer is already in this connection path. Return to its tab instead."
                        .into(),
                );
            }
        }
        Ok(())
    }
    pub fn value(&self) -> serde_json::Value {
        serde_json::json!(self)
    }
    pub fn append(&self, mut next: Self) -> Result<Self, String> {
        let mut parent = self.clone();
        let controllers = std::mem::take(&mut parent.controllers);
        next.controllers = controllers;
        next.controllers.push(parent);
        next.validate()?;
        Ok(next)
    }
    pub fn parent(&self) -> Option<Self> {
        let mut controllers = self.controllers.clone();
        let mut parent = controllers.pop()?;
        parent.controllers = controllers;
        Some(parent)
    }
}
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
/// Inspect transport controls without allocating a second copy of RPC payloads.
#[derive(Deserialize)]
pub struct Control<'a> {
    #[serde(borrow)]
    pub protocol: Option<&'a str>,
    #[serde(borrow)]
    pub ping: Option<&'a str>,
    #[serde(borrow)]
    pub pong: Option<&'a str>,
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

#[cfg(test)]
mod target_tests {
    use super::*;
    use serde_json::json;
    fn target(id: &str, subject: &str) -> ComputerTarget {
        ComputerTarget::parse(&json!({"environmentId":id,"expectedSubject":subject,"label":id}))
            .unwrap()
    }
    #[test]
    fn public_controller_path_round_trips_and_refuses_cycles_or_private_fields() {
        let root = target("host-a", "account-a");
        let nested = root.append(target("host-b", "account-b")).unwrap();
        let restored = ComputerTarget::parse(&nested.value()).unwrap();
        assert_eq!(restored.subject, "account-b");
        assert_eq!(restored.controllers[0].subject, "account-a");
        assert_eq!(restored.parent().unwrap().value(), root.value());
        assert!(restored.append(root.clone()).is_err());
        let third = restored.append(target("host-c", "account-c")).unwrap();
        let fourth = third.append(target("host-d", "account-d")).unwrap();
        assert!(fourth.append(target("host-e", "account-e")).is_err());
        let mut malformed = nested.value();
        malformed["controllers"][0]["controllers"] = json!([root.value()]);
        assert!(ComputerTarget::parse(&malformed).is_err());
        let mut private = root.value();
        private["connectToken"] = json!("not-public");
        assert!(ComputerTarget::parse(&private).is_err());
    }
}
