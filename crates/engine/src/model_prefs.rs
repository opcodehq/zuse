//! Per-device model choices for each provider: models the user hid from the
//! pickers, and model IDs they added by hand (a new release the CLI's catalog
//! doesn't list yet, a proxy's own model names, a pinned snapshot).
//!
//! Stored in `{data_dir}/model-prefs.json`. Every catalog the engine serves
//! goes through [`ModelPrefs::apply`], so the composer, defaults and titles all
//! see the same list; Settings reads the unfiltered view from
//! [`ModelPrefs::entries`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zuse_protocol::{HarnessId, Model, ProviderModel, ProviderModelPrefs, ReasoningLevel};

use crate::EngineError;

/// The longest model ID accepted — real IDs are short; this only keeps a
/// pasted paragraph out of every catalog.
const MAX_ID_LEN: usize = 200;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    #[serde(default)]
    providers: BTreeMap<String, ProviderModelPrefs>,
}

#[derive(Clone)]
pub struct ModelPrefs {
    path: Arc<PathBuf>,
    file: Arc<Mutex<File>>,
}

fn key(harness: HarnessId) -> String {
    serde_json::to_value(harness)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl ModelPrefs {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("model-prefs.json");
        let file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            path: Arc::new(path),
            file: Arc::new(Mutex::new(file)),
        }
    }

    pub fn get(&self, harness: HarnessId) -> ProviderModelPrefs {
        self.lock()
            .providers
            .get(&key(harness))
            .cloned()
            .unwrap_or_default()
    }

    /// Replace one provider's choices. IDs are trimmed and de-duplicated; a
    /// custom ID that is also hidden stays hidden.
    pub fn set(&self, harness: HarnessId, prefs: ProviderModelPrefs) -> Result<(), EngineError> {
        let prefs = normalize(prefs)?;
        let mut file = self.lock().clone();
        if prefs.hidden.is_empty() && prefs.custom.is_empty() {
            file.providers.remove(&key(harness));
        } else {
            file.providers.insert(key(harness), prefs);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| EngineError::Other(format!("serialize model prefs: {e}")))?;
        let staging = self
            .path
            .with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
        std::fs::write(&staging, json)?;
        std::fs::rename(&staging, self.path.as_ref()).inspect_err(|_| {
            let _ = std::fs::remove_file(&staging);
        })?;
        *self.lock() = file;
        Ok(())
    }

    /// The catalog a picker shows: hidden models removed, custom IDs appended
    /// (offering the provider's reasoning levels, since the CLI decides).
    pub fn apply(
        &self,
        harness: HarnessId,
        mut models: Vec<Model>,
        reasoning: &[ReasoningLevel],
    ) -> Vec<Model> {
        let prefs = self.get(harness);
        if prefs.hidden.is_empty() && prefs.custom.is_empty() {
            return models;
        }
        for custom in &prefs.custom {
            if !models.iter().any(|m| m.id == custom.id) {
                models.push(Model {
                    id: custom.id.clone(),
                    label: custom.label.clone().unwrap_or_else(|| custom.id.clone()),
                    description: Some("Added by you".into()),
                    reasoning_levels: reasoning.to_vec(),
                    options: Vec::new(),
                });
            }
        }
        let filtered: Vec<Model> = models
            .iter()
            .filter(|m| !prefs.hidden.contains(&m.id))
            .cloned()
            .collect();
        // Never leave a provider with nothing to pick.
        if filtered.is_empty() {
            models
        } else {
            filtered
        }
    }

    /// Settings' view: every model with whether it is custom or hidden.
    pub fn entries(&self, harness: HarnessId, models: &[Model]) -> Vec<ProviderModel> {
        let prefs = self.get(harness);
        let mut out: Vec<ProviderModel> = models
            .iter()
            .map(|m| ProviderModel {
                id: m.id.clone(),
                label: m.label.clone(),
                description: m.description.clone(),
                custom: false,
                hidden: prefs.hidden.contains(&m.id),
            })
            .collect();
        for custom in &prefs.custom {
            match out.iter_mut().find(|m| m.id == custom.id) {
                // The CLI caught up and lists it now; keep it removable.
                Some(existing) => existing.custom = true,
                None => out.push(ProviderModel {
                    id: custom.id.clone(),
                    label: custom.label.clone().unwrap_or_else(|| custom.id.clone()),
                    description: None,
                    custom: true,
                    hidden: prefs.hidden.contains(&custom.id),
                }),
            }
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn normalize(mut prefs: ProviderModelPrefs) -> Result<ProviderModelPrefs, EngineError> {
    let mut seen = std::collections::HashSet::new();
    let mut custom = Vec::new();
    for mut model in prefs.custom {
        model.id = model.id.trim().to_owned();
        model.label = model
            .label
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty());
        if model.id.is_empty() {
            continue;
        }
        if model.id.len() > MAX_ID_LEN || model.id.chars().any(char::is_whitespace) {
            return Err(EngineError::Other(format!(
                "“{}” isn't a model ID — IDs have no spaces.",
                model.id
            )));
        }
        if seen.insert(model.id.clone()) {
            custom.push(model);
        }
    }
    prefs.custom = custom;
    prefs.hidden = prefs
        .hidden
        .into_iter()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(prefs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::CustomModel;

    fn model(id: &str) -> Model {
        Model {
            id: id.into(),
            label: id.to_uppercase(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }
    }

    #[test]
    fn hides_adds_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = ModelPrefs::open(dir.path());
        let catalog = vec![model("a"), model("b")];
        assert_eq!(
            prefs
                .apply(HarnessId::ClaudeCode, catalog.clone(), &[])
                .len(),
            2
        );
        prefs
            .set(
                HarnessId::ClaudeCode,
                ProviderModelPrefs {
                    hidden: vec!["b".into(), " b ".into()],
                    custom: vec![
                        CustomModel {
                            id: " my-model ".into(),
                            label: None,
                        },
                        CustomModel {
                            id: "my-model".into(),
                            label: Some("dup".into()),
                        },
                    ],
                },
            )
            .unwrap();
        let shown: Vec<_> = prefs
            .apply(
                HarnessId::ClaudeCode,
                catalog.clone(),
                &[ReasoningLevel::High],
            )
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(shown, ["a", "my-model"]);
        // Other providers are untouched.
        assert_eq!(prefs.apply(HarnessId::Codex, catalog.clone(), &[]).len(), 2);
        // Survives a restart, and Settings sees every model with its flags.
        let reopened = ModelPrefs::open(dir.path());
        let entries = reopened.entries(HarnessId::ClaudeCode, &catalog);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.id == "b" && e.hidden && !e.custom));
        assert!(
            entries
                .iter()
                .any(|e| e.id == "my-model" && e.custom && !e.hidden)
        );
    }

    #[test]
    fn rejects_ids_with_spaces_and_never_empties_a_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = ModelPrefs::open(dir.path());
        let bad = ProviderModelPrefs {
            hidden: vec![],
            custom: vec![CustomModel {
                id: "not an id".into(),
                label: None,
            }],
        };
        assert!(prefs.set(HarnessId::ClaudeCode, bad).is_err());
        prefs
            .set(
                HarnessId::ClaudeCode,
                ProviderModelPrefs {
                    hidden: vec!["a".into()],
                    custom: vec![],
                },
            )
            .unwrap();
        assert_eq!(
            prefs
                .apply(HarnessId::ClaudeCode, vec![model("a")], &[])
                .len(),
            1
        );
    }
}
