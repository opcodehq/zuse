//! Server-owned plugin catalog. Clients receive display metadata only; endpoints,
//! OAuth discovery targets and scopes never leave the API.
//!
//! `catalog.json` was converted from the Workers API's generated catalog
//! (integrations.sh feed). Changing an entry's endpoint or scopes requires
//! existing connections to that entry to reconnect.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::LazyLock};
use zuse_protocol::plugins::PluginDefinition;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    None,
    Oauth,
    Header,
}
impl Auth {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Oauth => "oauth",
            Self::Header => "header",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "oauth" => Some(Self::Oauth),
            "header" => Some(Self::Header),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub category: Option<String>,
    pub featured: bool,
    pub endpoint: String,
    #[serde(default)]
    pub oauth_discovery: Option<String>,
    /// Feed hint. Without it, auth is probed when connecting.
    #[serde(default)]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
}

struct Catalog {
    entries: Vec<CatalogPlugin>,
    by_id: HashMap<String, usize>,
    public: Vec<PluginDefinition>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let entries: Vec<CatalogPlugin> =
        serde_json::from_str(include_str!("catalog.json")).expect("valid plugin catalog");
    let by_id = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.clone(), index))
        .collect();
    let public = entries
        .iter()
        .map(|entry| PluginDefinition {
            command: None,
            id: entry.id.clone(),
            name: entry.name.clone(),
            description: entry.description.clone(),
            domain: entry.domain.clone(),
            category: entry.category.clone(),
            featured: entry.featured,
            custom: false,
            url: None,
            api_key_header: None,
        })
        .collect();
    Catalog {
        entries,
        by_id,
        public,
    }
});

pub fn find(id: &str) -> Option<&'static CatalogPlugin> {
    let catalog = &*CATALOG;
    catalog.by_id.get(id).map(|index| &catalog.entries[*index])
}

pub fn public() -> &'static [PluginDefinition] {
    &CATALOG.public
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_unique_and_policy_compliant() {
        let catalog = &*CATALOG;
        assert!(catalog.entries.len() > 1000);
        assert_eq!(catalog.by_id.len(), catalog.entries.len());
        for entry in &catalog.entries {
            assert!(
                super::super::egress::allowed(&entry.endpoint),
                "{}",
                entry.id
            );
            if let Some(discovery) = &entry.oauth_discovery {
                assert!(super::super::egress::allowed(discovery), "{}", entry.id);
            }
        }
        assert_eq!(find("linear").unwrap().auth, Some(Auth::Oauth));
        assert!(
            public()
                .iter()
                .all(|entry| !entry.custom && entry.url.is_none())
        );
    }

    #[test]
    fn every_bundled_client_entry_is_served() {
        let bundled: Vec<PluginDefinition> =
            serde_json::from_str(include_str!("../../../protocol/src/plugin-catalog.json"))
                .unwrap();
        for entry in bundled {
            assert!(find(&entry.id).is_some(), "{}", entry.id);
        }
    }
}
