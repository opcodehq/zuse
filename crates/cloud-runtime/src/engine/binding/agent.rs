//! Resolve the selected managed harness from authenticated server policy.
use crate::mailbox::{reject, require};
use anyhow::Result;
use zuse_protocol::{HarnessId, RunRequest, cloud::CloudRuntimeBootstrap};

fn agent_key(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::ClaudeCode => Some("claude"),
        HarnessId::Codex => Some("codex"),
        HarnessId::Grok => Some("grok"),
        _ => None,
    }
}

pub(super) fn resolve(
    boot: &CloudRuntimeBootstrap,
    request: &RunRequest,
    installed: impl FnOnce(HarnessId) -> bool,
) -> Result<HarnessId> {
    let initial = match boot.extra.get("agent").and_then(serde_json::Value::as_str) {
        Some("claude" | "claude-code") => HarnessId::ClaudeCode,
        Some("codex") => HarnessId::Codex,
        Some("grok") => HarnessId::Grok,
        _ => return Err(reject("cloud_command_agent_changed")),
    };
    let selected = request
        .harness
        .ok_or_else(|| reject("cloud_command_agent_changed"))?;
    let selected_key = agent_key(selected).ok_or_else(|| reject("cloud_command_agent_changed"))?;
    if let Some(value) = boot.extra.get("managedAgentCatalogs") {
        let catalogs: std::collections::BTreeMap<
            String,
            zuse_protocol::cloud_catalog::CloudAgentModels,
        > = serde_json::from_value(value.clone())
            .map_err(|_| reject("cloud_command_agent_catalog_invalid"))?;
        require(
            catalogs.contains_key(agent_key(initial).unwrap()),
            "cloud_command_agent_catalog_invalid",
        )?;
        for (key, catalog) in &catalogs {
            require(
                agent_key(catalog.id) == Some(key.as_str())
                    && !catalog.models.is_empty()
                    && catalog.resolve_model(&catalog.default_model).is_some(),
                "cloud_command_agent_catalog_invalid",
            )?;
        }
        let catalog = catalogs
            .get(selected_key)
            .ok_or_else(|| reject("cloud_command_agent_changed"))?;
        require(
            request.model.as_deref().is_some_and(|model| {
                !model.trim().is_empty() && catalog.resolve_model(model).is_some()
            }),
            "cloud_command_model_changed",
        )?;
    } else {
        // A legacy guest/task with no sealed catalogue remains single-agent.
        require(selected == initial, "cloud_command_agent_changed")?;
    }
    // Resolve/check only the requested installed harness, never other credentials.
    require(installed(selected), "cloud_command_agent_not_installed")?;
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use zuse_protocol::cloud_catalog::CloudAgentModels;
    fn catalog(id: HarnessId, model: &str) -> CloudAgentModels {
        serde_json::from_value(json!({"id":id,"name":"Controlled fixture","defaultModel":model,"models":[{"id":model,"label":"Controlled model"}]})).unwrap()
    }
    fn boot() -> CloudRuntimeBootstrap {
        serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9999999999999i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","agent":"codex","managedAgentCatalogs":{"codex":catalog(HarnessId::Codex,"codex-model"),"claude":catalog(HarnessId::ClaudeCode,"claude-model"),"grok":catalog(HarnessId::Grok,"grok-model")}})).unwrap()
    }
    fn request(harness: HarnessId, model: &str) -> RunRequest {
        serde_json::from_value(json!({"prompt":"controlled","harness":harness,"model":model,"cwd":".","sandbox":"workspace-write"})).unwrap()
    }
    #[test]
    fn server_catalog_allows_codex_claude_grok_codex_without_changing_boot_agent() {
        let boot = boot();
        let seen = RefCell::new(Vec::new());
        for (harness, model) in [
            (HarnessId::Codex, "codex-model"),
            (HarnessId::ClaudeCode, "claude-model"),
            (HarnessId::Grok, "grok-model"),
            (HarnessId::Codex, "codex-model"),
        ] {
            assert_eq!(
                resolve(&boot, &request(harness, model), |selected| {
                    seen.borrow_mut().push(selected);
                    true
                })
                .unwrap(),
                harness
            );
        }
        assert_eq!(
            *seen.borrow(),
            vec![
                HarnessId::Codex,
                HarnessId::ClaudeCode,
                HarnessId::Grok,
                HarnessId::Codex
            ]
        );
        assert_eq!(boot.extra["agent"], "codex");
    }
    #[test]
    fn legacy_or_single_agent_catalog_does_not_expand_authority() {
        let mut boot = boot();
        boot.extra.remove("managedAgentCatalogs");
        assert_eq!(
            resolve(
                &boot,
                &request(HarnessId::Codex, "legacy-server-model"),
                |_| true
            )
            .unwrap(),
            HarnessId::Codex
        );
        assert!(
            resolve(
                &boot,
                &request(HarnessId::ClaudeCode, "claude-model"),
                |_| true
            )
            .is_err()
        );
        boot.extra.insert(
            "managedAgentCatalogs".into(),
            json!({"codex":catalog(HarnessId::Codex,"codex-model")}),
        );
        assert!(
            resolve(
                &boot,
                &request(HarnessId::ClaudeCode, "claude-model"),
                |_| true
            )
            .is_err()
        );
    }
    #[test]
    fn invalid_catalog_keys_ids_models_and_missing_expected_are_rejected() {
        let original = boot();
        for value in [
            json!({}),
            json!({"claude":catalog(HarnessId::ClaudeCode,"claude-model")}),
            json!({"codex":catalog(HarnessId::ClaudeCode,"codex-model")}),
            json!({"codex":catalog(HarnessId::Codex,"codex-model"),"mock":catalog(HarnessId::Mock,"mock-model")}),
            json!(null),
        ] {
            let mut boot = original.clone();
            boot.extra.insert("managedAgentCatalogs".into(), value);
            assert!(resolve(&boot, &request(HarnessId::Codex, "codex-model"), |_| true).is_err());
        }
        assert!(
            resolve(
                &original,
                &request(HarnessId::ClaudeCode, "codex-model"),
                |_| true
            )
            .is_err()
        );
        assert!(
            resolve(&original, &request(HarnessId::Mock, "codex-model"), |_| {
                true
            })
            .is_err()
        );
        let mut missing = original.clone();
        missing.extra.remove("agent");
        assert!(
            resolve(&missing, &request(HarnessId::Codex, "codex-model"), |_| {
                true
            })
            .is_err()
        );
        let mut missing = request(HarnessId::Codex, "codex-model");
        missing.harness = None;
        assert!(resolve(&original, &missing, |_| true).is_err());
        missing.harness = Some(HarnessId::Codex);
        missing.model = None;
        assert!(resolve(&original, &missing, |_| true).is_err());
    }
    #[test]
    fn registered_but_uninstalled_selected_harness_is_rejected_without_fallback() {
        let seen = RefCell::new(Vec::new());
        assert!(
            resolve(
                &boot(),
                &request(HarnessId::Grok, "grok-model"),
                |selected| {
                    seen.borrow_mut().push(selected);
                    false
                }
            )
            .is_err()
        );
        assert_eq!(*seen.borrow(), vec![HarnessId::Grok]);
    }
    #[test]
    fn agent_refusals_are_definitive_because_they_only_read_policy() {
        use crate::mailbox::AdmissionRejected;
        let code = |result: Result<HarnessId>| {
            result
                .unwrap_err()
                .downcast_ref::<AdmissionRejected>()
                .map(|r| r.0)
        };
        assert_eq!(
            code(resolve(
                &boot(),
                &request(HarnessId::Grok, "grok-model"),
                |_| false
            )),
            Some("cloud_command_agent_not_installed")
        );
        assert_eq!(
            code(resolve(
                &boot(),
                &request(HarnessId::Codex, "unknown-model"),
                |_| true
            )),
            Some("cloud_command_model_changed")
        );
        assert_eq!(
            code(resolve(
                &boot(),
                &request(HarnessId::Mock, "codex-model"),
                |_| true
            )),
            Some("cloud_command_agent_changed")
        );
    }
}
