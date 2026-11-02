//! Published token rates. Estimates belong to report projections, never the raw ledger.
mod cache;
pub use cache::Pricing;

use crate::{Data, TokenCounts};
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, OnceLock},
};

const PREFIXES: &[&str] = &[
    "anthropic/",
    "claude-3-5-",
    "claude-3-",
    "claude-",
    "openai/",
    "azure/",
    "openrouter/openai/",
    "xai/",
    "gemini/",
    "vertex_ai/",
    "mistral/",
    "deepseek/",
    "openrouter/",
];
const FIELDS: [&str; 4] = [
    "input_cost_per_token",
    "output_cost_per_token",
    "cache_creation_input_token_cost",
    "cache_read_input_token_cost",
];
const THRESHOLD: u64 = 200_000;

#[derive(Debug)]
struct Rates {
    base: [Option<f64>; 4],
    above_200k: [Option<f64>; 4],
    fast: f64,
}
fn rate(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite() && *v >= 0.)
}
impl Rates {
    fn parse(raw: Value) -> Option<Self> {
        let base = FIELDS.map(|field| rate(&raw[field]));
        let above_200k = FIELDS.map(|field| rate(&raw[format!("{field}_above_200k_tokens")]));
        // Capacity metadata alone is not evidence that a model is free.
        if base.iter().chain(&above_200k).all(Option::is_none) {
            return None;
        }
        Some(Self {
            base,
            above_200k,
            fast: rate(&raw["provider_specific_entry"]["fast"]).unwrap_or(1.),
        })
    }
    fn cost(&self, tokens: &TokenCounts, fast: bool) -> Option<f64> {
        let counts = [
            tokens.input_tokens,
            tokens.output_tokens,
            tokens.cache_creation_tokens,
            tokens.cache_read_tokens,
        ];
        let cost: f64 = counts
            .into_iter()
            .enumerate()
            .map(|(i, count)| {
                if count > THRESHOLD
                    && let Some(tier) = self.above_200k[i]
                {
                    THRESHOLD as f64 * self.base[i].unwrap_or(0.)
                        + (count - THRESHOLD) as f64 * tier
                } else {
                    count as f64 * self.base[i].unwrap_or(0.)
                }
            })
            .sum();
        let cost = cost * if fast { self.fast } else { 1. };
        (cost.is_finite() && cost >= 0.).then_some(cost)
    }
}

/// Preserve source order: equally close fuzzy matches use the first catalogue entry.
#[derive(Debug)]
pub struct Catalogue {
    entries: Vec<(String, Rates)>,
    index: HashMap<String, usize>,
}
impl<'de> Deserialize<'de> for Catalogue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Catalogue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a nonempty model pricing catalogue")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Catalogue, M::Error> {
                let mut table = Catalogue {
                    entries: Vec::new(),
                    index: HashMap::new(),
                };
                while let Some((name, raw)) = map.next_entry::<String, Value>()? {
                    if let Some(rates) = Rates::parse(raw) {
                        if let Some(i) = table.index.get(&name).copied() {
                            table.entries[i].1 = rates;
                        } else {
                            table.index.insert(name.clone(), table.entries.len());
                            table.entries.push((name, rates));
                        }
                    }
                }
                if table.entries.is_empty() {
                    return Err(serde::de::Error::custom(
                        "catalogue contains no token rates",
                    ));
                }
                Ok(table)
            }
        }
        deserializer.deserialize_map(Ordered)
    }
}
impl Catalogue {
    pub fn parse(json: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(json)
    }
    pub fn bundled() -> Arc<Self> {
        static BUNDLED: OnceLock<Arc<Catalogue>> = OnceLock::new();
        BUNDLED
            .get_or_init(|| {
                Arc::new(
                    Self::parse(include_bytes!("../data/model-prices.json"))
                        .expect("bundled model pricing must be valid"),
                )
            })
            .clone()
    }
    fn find(&self, model: &str) -> Option<usize> {
        let name = model.trim();
        if name.is_empty() || name == "unknown" {
            return None;
        }
        let name = if name == "grok-build" {
            "grok-code-fast-1"
        } else {
            name
        };
        if let Some(i) = self.index.get(name) {
            return Some(*i);
        }
        for prefix in PREFIXES {
            if let Some(i) = self.index.get(&format!("{prefix}{name}")) {
                return Some(*i);
            }
        }
        let lower = name.to_lowercase();
        let length = lower.encode_utf16().count();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, (key, _))| {
                let key = key.to_lowercase();
                (key.contains(&lower) || lower.contains(&key))
                    .then(|| (i, key.encode_utf16().count().abs_diff(length)))
            })
            .min_by_key(|(_, difference)| *difference)
            .map(|(i, _)| i)
    }
    pub fn estimate(&self, model: &str, tokens: &TokenCounts, fast: bool) -> Option<f64> {
        self.entries.get(self.find(model)?)?.1.cost(tokens, fast)
    }
    pub fn apply(&self, data: &mut Data) {
        let mut matches = HashMap::new();
        for record in &mut data.records {
            if record.cost_usd.is_some() {
                continue;
            }
            let index = matches
                .entry(record.model.as_str())
                .or_insert_with(|| self.find(&record.model));
            record.cost_usd =
                index.and_then(|i| self.entries[i].1.cost(&record.tokens, record.fast));
        }
    }
}
