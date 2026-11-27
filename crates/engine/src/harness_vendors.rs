//! Built-in API-key connections for the Zuse agent. Kimi and GLM both serve an
//! Anthropic Messages endpoint, so they reuse the native Messages transport.
//! Keys are saved owner-only beside `providers.json`, or read from the
//! vendor's environment variable when none is saved. A key never leaves the
//! engine.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use zeroize::Zeroizing;
use zuse_harness::transport::Authentication;
use zuse_protocol::ModelApiKey;

pub(crate) struct Vendor {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub endpoint: &'static str,
    pub authentication: Authentication,
    pub credential_env: &'static str,
    /// Billed to a coding plan rather than per token.
    pub subscription: bool,
    pub console_url: &'static str,
    pub models: &'static [VendorModel],
}

pub(crate) struct VendorModel {
    pub id: &'static str,
    pub label: &'static str,
    pub context_window_tokens: u64,
    pub images: bool,
}

pub(crate) const MAX_OUTPUT_TOKENS: u32 = 32768;

pub(crate) const VENDORS: &[Vendor] = &[
    Vendor {
        id: "kimi",
        label: "Kimi API",
        description: "Pay-as-you-go key from the Kimi API Platform (Moonshot AI).",
        endpoint: "https://api.moonshot.ai/anthropic/v1/messages",
        authentication: Authentication::Bearer,
        credential_env: "MOONSHOT_API_KEY",
        subscription: false,
        console_url: "https://platform.kimi.ai/docs/get-your-api-key",
        models: &[
            VendorModel {
                id: "kimi-k3.1-pro",
                label: "Kimi K3.1 Pro",
                context_window_tokens: 1_048_576,
                images: true,
            },
            VendorModel {
                id: "kimi-k3.1-pro-highspeed",
                label: "Kimi K3.1 Pro Highspeed",
                context_window_tokens: 1_048_576,
                images: true,
            },
            VendorModel {
                id: "kimi-k3",
                label: "Kimi K3",
                context_window_tokens: 1_048_576,
                images: true,
            },
        ],
    },
    Vendor {
        id: "kimi-code",
        label: "Kimi Code",
        description: "Key from the Kimi Code console, billed to your Kimi membership.",
        endpoint: "https://api.kimi.ai/coding/v1/messages",
        authentication: Authentication::ProtocolDefault,
        credential_env: "KIMI_CODE_API_KEY",
        subscription: true,
        console_url: "https://www.kimi.com/code/docs/en/",
        // Below its top tiers the coding plan caps requests at 256K tokens.
        models: &[
            VendorModel {
                id: "k3.1-pro",
                label: "Kimi K3.1 Pro · Kimi Code",
                context_window_tokens: 262_144,
                images: true,
            },
            VendorModel {
                id: "k3.1-pro-highspeed",
                label: "Kimi K3.1 Pro Highspeed · Kimi Code",
                context_window_tokens: 262_144,
                images: true,
            },
            VendorModel {
                id: "k3",
                label: "Kimi K3 · Kimi Code",
                context_window_tokens: 262_144,
                images: true,
            },
        ],
    },
    Vendor {
        id: "glm",
        label: "GLM (Z.ai)",
        description: "Standard API key or GLM Coding Plan key from Z.ai; the key decides billing.",
        endpoint: "https://api.z.ai/api/anthropic/v1/messages",
        authentication: Authentication::Bearer,
        credential_env: "ZAI_API_KEY",
        subscription: false,
        console_url: "https://z.ai/manage-apikey/apikey-list",
        models: &[
            VendorModel {
                id: "glm-5.4",
                label: "GLM-5.4 (Coding Plan)",
                context_window_tokens: 1_000_000,
                images: true,
            },
            VendorModel {
                id: "glm-5.3",
                label: "GLM-5.3",
                context_window_tokens: 1_000_000,
                images: false,
            },
            VendorModel {
                id: "glm-5.3-flash",
                label: "GLM-5.3 Flash",
                context_window_tokens: 1_000_000,
                images: true,
            },
        ],
    },
];

pub(crate) fn vendor(id: &str) -> Option<&'static Vendor> {
    VENDORS.iter().find(|v| v.id == id)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyError {
    UnknownConnection,
    InvalidKey,
    Storage,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownConnection => "Unknown API connection.",
            Self::InvalidKey => "Paste the whole API key, without spaces or line breaks.",
            Self::Storage => "The API key could not be saved.",
        })
    }
}

/// Saved vendor keys. Cloning shares the same store.
#[derive(Clone, Default)]
pub(crate) struct ApiKeys {
    path: Option<PathBuf>,
    saved: Arc<RwLock<BTreeMap<String, Zeroizing<String>>>>,
}

impl ApiKeys {
    /// An unreadable store starts empty rather than blocking the engine.
    pub fn open(path: PathBuf) -> Self {
        let saved = std::fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() <= 64 * 1024)
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, String>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, key)| vendor(id).is_some() && valid_key(key))
            .map(|(id, key)| (id, Zeroizing::new(key)))
            .collect();
        Self {
            path: Some(path),
            saved: Arc::new(RwLock::new(saved)),
        }
    }

    fn saved(&self, id: &str) -> Option<Zeroizing<String>> {
        self.saved
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// The saved key, else the vendor's environment variable.
    pub fn key(&self, vendor: &Vendor) -> Option<Zeroizing<String>> {
        self.saved(vendor.id).or_else(|| {
            std::env::var(vendor.credential_env)
                .ok()
                .filter(|key| valid_key(key))
                .map(Zeroizing::new)
        })
    }

    /// Saves `key` for the vendor, or removes the saved key when it is empty.
    pub fn set(&self, id: &str, key: &str) -> Result<(), KeyError> {
        vendor(id).ok_or(KeyError::UnknownConnection)?;
        let key = key.trim();
        if !key.is_empty() && !valid_key(key) {
            return Err(KeyError::InvalidKey);
        }
        let mut saved = self.saved.write().unwrap_or_else(|e| e.into_inner());
        let mut next = saved.clone();
        if key.is_empty() {
            next.remove(id);
        } else {
            next.insert(id.to_owned(), Zeroizing::new(key.to_owned()));
        }
        if let Some(path) = &self.path {
            persist(path, &next)?;
        }
        *saved = next;
        Ok(())
    }

    pub fn status(&self) -> Vec<ModelApiKey> {
        VENDORS
            .iter()
            .map(|v| {
                let has_key = self.saved(v.id).is_some();
                ModelApiKey {
                    id: v.id.into(),
                    label: v.label.into(),
                    description: v.description.into(),
                    has_key,
                    environment_variable: v.credential_env.into(),
                    from_environment: !has_key && self.key(v).is_some(),
                    models: v.models.iter().map(|m| m.label.into()).collect(),
                    console_url: v.console_url.into(),
                }
            })
            .collect()
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 1024 && key.bytes().all(|b| b.is_ascii_graphic())
}

fn persist(path: &Path, keys: &BTreeMap<String, Zeroizing<String>>) -> Result<(), KeyError> {
    use std::io::Write;
    let directory = path.parent().ok_or(KeyError::Storage)?;
    std::fs::create_dir_all(directory).map_err(|_| KeyError::Storage)?;
    let plain: BTreeMap<&str, &str> = keys
        .iter()
        .map(|(id, key)| (id.as_str(), key.as_str()))
        .collect();
    let bytes = Zeroizing::new(serde_json::to_vec(&plain).map_err(|_| KeyError::Storage)?);
    // Temp files are created owner-only, so the key is never world-readable.
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| KeyError::Storage)?;
    file.write_all(&bytes).map_err(|_| KeyError::Storage)?;
    file.as_file().sync_all().map_err(|_| KeyError::Storage)?;
    file.persist(path).map_err(|_| KeyError::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors_use_https_messages_endpoints_and_unique_model_ids() {
        for vendor in VENDORS {
            assert!(vendor.endpoint.starts_with("https://"));
            assert!(vendor.endpoint.ends_with("/v1/messages"));
            assert!(!vendor.models.is_empty());
            let mut ids = std::collections::HashSet::new();
            for model in vendor.models {
                assert!(ids.insert(model.id), "{} repeats {}", vendor.id, model.id);
                assert!(model.context_window_tokens >= 200_000);
            }
        }
        let kimi = vendor("kimi").unwrap();
        assert_eq!(
            kimi.endpoint,
            "https://api.moonshot.ai/anthropic/v1/messages"
        );
        assert!(kimi.authentication == Authentication::Bearer);
        assert_eq!(kimi.models[0].id, "kimi-k3.1-pro");
        assert_eq!(kimi.models[0].context_window_tokens, 1_048_576);
        let code = vendor("kimi-code").unwrap();
        assert_eq!(code.endpoint, "https://api.kimi.ai/coding/v1/messages");
        assert!(code.subscription);
        let glm = vendor("glm").unwrap();
        assert_eq!(glm.endpoint, "https://api.z.ai/api/anthropic/v1/messages");
        assert!(glm.authentication == Authentication::Bearer);
        assert!(glm.models.iter().any(|m| m.id == "glm-5.3" && !m.images));
    }

    #[test]
    fn saved_keys_persist_owner_only_and_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("harness").join("api-keys.json");
        let keys = ApiKeys::open(path.clone());
        assert!(keys.status().iter().all(|row| !row.has_key));
        keys.set("glm", "  zai-key-123  ").unwrap();
        assert_eq!(
            keys.key(vendor("glm").unwrap())
                .as_deref()
                .map(String::as_str),
            Some("zai-key-123")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        let reopened = ApiKeys::open(path.clone());
        let row = reopened
            .status()
            .into_iter()
            .find(|r| r.id == "glm")
            .unwrap();
        assert!(row.has_key);
        assert_eq!(row.environment_variable, "ZAI_API_KEY");
        assert!(!serde_json::to_string(&row).unwrap().contains("zai-key-123"));
        reopened.set("glm", "").unwrap();
        assert!(!ApiKeys::open(path).status().iter().any(|r| r.has_key));
    }

    #[test]
    fn invalid_keys_and_unknown_connections_are_rejected() {
        let keys = ApiKeys::default();
        assert_eq!(keys.set("openai", "sk-1"), Err(KeyError::UnknownConnection));
        assert_eq!(keys.set("kimi", "two words"), Err(KeyError::InvalidKey));
        assert_eq!(keys.set("kimi", "line\nbreak"), Err(KeyError::InvalidKey));
        keys.set("kimi", "sk-kimi").unwrap();
        assert!(keys.status().iter().any(|r| r.id == "kimi" && r.has_key));
    }

    #[test]
    fn damaged_or_foreign_store_entries_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-keys.json");
        std::fs::write(&path, br#"{"kimi":"sk-ok","other":"sk-x","glm":"bad key"}"#).unwrap();
        let keys = ApiKeys::open(path.clone());
        let saved: Vec<_> = keys.status().into_iter().filter(|r| r.has_key).collect();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "kimi");
        std::fs::write(&path, b"not json").unwrap();
        assert!(ApiKeys::open(path).status().iter().all(|r| !r.has_key));
    }
}
