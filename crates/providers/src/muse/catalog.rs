//! `model/list` → the picker's models. Muse ships no bundled catalog, so the
//! signed-in account's live list is the only source.
use serde_json::Value;
use zuse_protocol::{Model, ReasoningLevel};

/// The provider routing Zuse drives; rows routed elsewhere are not offered.
pub(super) const PROVIDER: &str = "meta";

/// Efforts offered when a row does not describe its own.
const FALLBACK_LEVELS: [ReasoningLevel; 5] = [
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
    ReasoningLevel::XHigh,
    ReasoningLevel::Max,
];

/// Parse a `model/list` result: Meta-routed rows only, first occurrence of
/// each id wins, the catalog default first.
pub(super) fn models(result: &Value) -> Vec<Model> {
    let mut seen = std::collections::HashSet::new();
    let mut models: Vec<(bool, Model)> = Vec::new();
    for row in result["models"].as_array().into_iter().flatten() {
        let Some(id) = row["modelId"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        if row["providerId"] != PROVIDER || !seen.insert(id.to_owned()) {
            continue;
        }
        let label = row["displayLabel"]
            .as_str()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(id);
        models.push((
            row["isDefault"] == true,
            Model {
                id: id.into(),
                label: label.into(),
                description: row["description"]
                    .as_str()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(str::to_owned),
                reasoning_levels: levels(&row["variants"]),
                options: vec![],
            },
        ));
    }
    // Stable: the default leads, the host's own order follows.
    models.sort_by_key(|(default, _)| !default);
    models.into_iter().map(|(_, model)| model).collect()
}

/// A row's efforts. Tiers Zuse has no level for (`none`) are dropped; an
/// absent or `"unknown"` set falls back to the common ladder.
fn levels(variants: &Value) -> Vec<ReasoningLevel> {
    match variants.as_array() {
        Some(tiers) => tiers
            .iter()
            .filter_map(Value::as_str)
            .filter_map(level)
            .collect(),
        None => FALLBACK_LEVELS.to_vec(),
    }
}

fn level(tier: &str) -> Option<ReasoningLevel> {
    Some(match tier {
        "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        _ => return None,
    })
}

/// The `turn/start` `reasoningEffort` for a picked level, when Muse has one.
pub(super) fn effort(level: Option<ReasoningLevel>) -> Option<&'static str> {
    Some(match level? {
        ReasoningLevel::Minimal => "minimal",
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh => "xhigh",
        ReasoningLevel::Max => "max",
        ReasoningLevel::Ultra => "ultra",
        ReasoningLevel::Ultracode | ReasoningLevel::Ultrathink => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_meta_rows_once_with_described_or_fallback_efforts() {
        let models = models(&json!({
            "providerId": "meta",
            "models": [
                {"modelId": "other", "displayLabel": "Other", "providerId": "other", "isDefault": false},
                {"modelId": "muse-fast", "displayLabel": "  ", "providerId": "meta", "isDefault": false, "variants": "unknown"},
                {"modelId": "muse-test", "displayLabel": "Muse Test", "providerId": "meta", "isDefault": true, "variants": ["none", "high", "max"]},
                {"modelId": "muse-test", "displayLabel": "Duplicate", "providerId": "meta", "isDefault": false},
                {"modelId": "", "displayLabel": "Empty", "providerId": "meta", "isDefault": false},
            ]
        }));
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["muse-test", "muse-fast"]);
        assert_eq!(models[0].label, "Muse Test");
        assert_eq!(
            models[0].reasoning_levels,
            [ReasoningLevel::High, ReasoningLevel::Max]
        );
        assert_eq!(models[1].label, "muse-fast");
        assert_eq!(models[1].reasoning_levels, FALLBACK_LEVELS);
    }

    #[test]
    fn efforts_round_trip_through_the_wire_vocabulary() {
        for tier in ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"] {
            assert_eq!(effort(level(tier)), Some(tier));
        }
        assert_eq!(effort(Some(ReasoningLevel::Ultrathink)), None);
        assert_eq!(effort(None), None);
    }
}
