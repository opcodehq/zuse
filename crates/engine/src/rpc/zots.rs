use super::*;
use zuse_protocol::zots::{ZotAction, ZotCommand};

impl EngineRpc {
    pub(super) async fn zot_command(
        &self,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let command: ZotCommand = parse_params(params)?;
        let zots = self
            .zots
            .as_ref()
            .ok_or_else(|| RpcError::Capability("zots.v1".into()))?;
        let failed = |e: anyhow::Error| RpcError::Failed(e.to_string());
        if let Some(result) = zots.receipt(&command).map_err(failed)? {
            return RpcReply::value(&result);
        }
        if let ZotAction::Create { config } | ZotAction::Configure { config } = &command.action {
            let _: HarnessId =
                serde_json::from_value(serde_json::json!(config.coordinator_model.harness))
                    .map_err(|e| RpcError::BadParams(e.to_string()))?;
            for repository in &config.repositories {
                let space = self
                    .workspace
                    .space(&repository.space_id)
                    .map_err(|e| RpcError::Failed(e.to_string()))?
                    .ok_or_else(|| RpcError::BadParams("repository is not registered".into()))?;
                if space.device_id != repository.device_id {
                    return Err(RpcError::BadParams("repository device mismatch".into()));
                }
            }
        }
        if let ZotAction::AdoptWorker { worker } = &command.action {
            let chat = self
                .workspace
                .chat(&worker.chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("worker conversation does not exist".into()))?;
            if chat.device_id != worker.device_id
                || chat.space_id.as_deref() != Some(&worker.space_id)
                || chat.parent_chat_id.is_some()
                || zots.coordinator(&chat.id).map_err(failed)?.is_some()
            {
                return Err(RpcError::BadParams(
                    "worker must be an ordinary conversation in the selected repository".into(),
                ));
            }
        }
        if let ZotAction::Delegate {
            worker,
            model,
            base,
        } = &command.action
        {
            if self
                .workspace
                .chat(&worker.chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .is_some()
            {
                return Err(RpcError::BadParams(
                    "delegation requires a new conversation identity".into(),
                ));
            }
            if worker.chat_id != format!("zot-worker-{}", command.command_id)
                || base.trim().is_empty()
            {
                return Err(RpcError::BadParams(
                    "delegate chatId must be zot-worker-<commandId>, with an explicit base ref"
                        .into(),
                ));
            }
            let _: HarnessId = serde_json::from_value(serde_json::json!(model.harness))
                .map_err(|e| RpcError::BadParams(e.to_string()))?;
        }
        let old = zots.get(&command.zot_id).ok();
        if matches!(
            command.action,
            ZotAction::FreshConversation | ZotAction::Delete
        ) && old
            .as_ref()
            .is_some_and(|z| self.sessions.turn_in_flight(&z.coordinator_chat_id))
        {
            return Err(RpcError::Failed(
                "stop the active coordinator turn first".into(),
            ));
        }
        let result = zots.command(command.clone()).map_err(failed)?;
        let current = result.as_ref().and_then(|z| zots.get(&z.id).ok());
        if let Some(zot) = &current {
            let harness: HarnessId =
                serde_json::from_value(serde_json::json!(zot.config.coordinator_model.harness))
                    .map_err(|e| RpcError::BadParams(e.to_string()))?;
            let cwd = zots
                .workspace(&zot.id)
                .map_err(failed)?
                .to_string_lossy()
                .into_owned();
            let previous = old
                .as_ref()
                .filter(|o| o.coordinator_chat_id != zot.coordinator_chat_id)
                .and_then(|o| self.workspace.chat_config(&o.coordinator_chat_id));
            self.workspace
                .create_chat_with_parent(
                    &zot.coordinator_chat_id,
                    None,
                    Some(&zot.config.home_device_id),
                    Some(zuse_protocol::ChatConfig {
                        harness: previous.as_ref().map_or(harness, |c| c.harness),
                        model: previous.as_ref().map_or_else(
                            || zot.config.coordinator_model.model.clone(),
                            |c| c.model.clone(),
                        ),
                        reasoning: None,
                        model_options: Default::default(),
                        sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
                        plan: true,
                    }),
                    Some(cwd),
                    None,
                )
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            self.workspace
                .rename_chat(&zot.coordinator_chat_id, &zot.config.name)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            let mut config = self
                .workspace
                .chat_config(&zot.coordinator_chat_id)
                .unwrap_or(zuse_protocol::ChatConfig {
                    harness,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
                    plan: true,
                });
            // The conversation's own model picker chooses the model after
            // creation; only an explicit model change through the API applies.
            if let ZotAction::Configure { config: wanted } = &command.action {
                if old
                    .as_ref()
                    .is_some_and(|o| o.config.coordinator_model != wanted.coordinator_model)
                {
                    config.harness = harness;
                    config.model = zot.config.coordinator_model.model.clone();
                }
            }
            config.sandbox = zuse_protocol::SandboxLevel::WorkspaceWrite;
            config.plan = true;
            self.workspace
                .set_chat_config(&zot.coordinator_chat_id, &config)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
        }
        RpcReply::value(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EngineCore, EngineProfile};
    use zuse_protocol::zots::*;

    #[tokio::test]
    async fn delegated_worker_runs_in_an_isolated_tree_and_reports_once() {
        use zuse_protocol::{AgentEvent, DoneStatus};
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "Initial",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let registry = std::sync::Arc::new(HarnessRegistry::new());
        registry.register(std::sync::Arc::new(zuse_providers::mock::MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "Worker report".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        let core = EngineCore::assemble_with_profile(
            EngineProfile::local(&dir.path().join("profile")).unwrap(),
            registry,
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let worktrees = dir.path().join("worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        core.repos
            .set_worktree_settings(zuse_protocol::WorktreeSettings {
                use_custom_directory: true,
                custom_directory: Some(worktrees.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();
        core.workspace
            .create_space("repo", &core.device_id, repo.to_str().unwrap(), None, true)
            .unwrap();
        let service = core.rpc_service();
        let config = ZotConfig {
            name: "Release".into(),
            purpose: "Coordinate".into(),
            home_device_id: core.device_id.clone(),
            repositories: vec![ZotRepository {
                device_id: core.device_id.clone(),
                space_id: "repo".into(),
            }],
            coordinator_model: ZotModel {
                harness: "mock".into(),
                model: None,
            },
            worker_device_id: core.device_id.clone(),
            max_concurrent_workers: 1,
            max_followups: 3,
        };
        value(&service,"ZotCommand",serde_json::json!({"commandId":"create","zotId":"release","expectedRevision":null,"action":{"type":"create","config":config}})).await;
        let launch = serde_json::json!({"commandId":"launch","zotId":"release","expectedRevision":null,"action":{"type":"delegate","worker":{"chatId":"zot-worker-launch","deviceId":core.device_id,"spaceId":"repo","objective":"Inspect the repository","followups":0},"model":{"harness":"mock","model":null},"base":"main"}});
        value(&service, "ZotCommand", launch.clone()).await;
        value(&service, "ZotCommand", launch).await;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if core
                    .zots
                    .events("release")
                    .unwrap()
                    .iter()
                    .any(|e| e.kind == "settled")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("worker should finish and produce a durable report");
        let chat = core.workspace.chat("zot-worker-launch").unwrap().unwrap();
        let cwd = std::path::PathBuf::from(chat.cwd.unwrap());
        assert_ne!(cwd, repo);
        assert!(cwd.starts_with(worktrees));
        let reports = core
            .zots
            .events("release")
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "settled")
            .count();
        assert_eq!(reports, 1);
        assert_eq!(core.zots.get("release").unwrap().workers.len(), 1);
        core.shutdown().await;
    }

    async fn value(
        service: &EngineRpc,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        match service.handle(method, params).await.unwrap() {
            RpcReply::Value(v) => v,
            _ => panic!("expected a unary reply"),
        }
    }
    #[tokio::test]
    async fn zot_api_receipts_files_and_auth_share_engine_authority() {
        let dir = tempfile::tempdir().unwrap();
        let profile = EngineProfile::local(dir.path()).unwrap();
        let store_root = profile.store_root().to_path_buf();
        let core = EngineCore::assemble_with_profile(
            profile,
            std::sync::Arc::new(HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let service = core.rpc_service();
        assert!(
            service
                .handle(
                    zuse_protocol::control::CALL,
                    serde_json::json!({"token":"wrong","method":"ControlInfo"})
                )
                .await
                .is_err()
        );
        let token = std::fs::read_to_string(store_root.join("zots/control-token")).unwrap();
        let info = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ControlInfo"}),
        )
        .await;
        assert_eq!(info["apiVersion"], 1);
        let config = ZotConfig {
            name: "Release".into(),
            purpose: "Coordinate".into(),
            home_device_id: core.device_id.clone(),
            repositories: vec![],
            coordinator_model: ZotModel {
                harness: "mock".into(),
                model: None,
            },
            worker_device_id: core.device_id.clone(),
            max_concurrent_workers: 4,
            max_followups: 3,
        };
        let command = ZotCommand {
            command_id: "create".into(),
            zot_id: "release".into(),
            expected_revision: None,
            action: ZotAction::Create { config },
        };
        let envelope = serde_json::json!({"token":token,"method":"ZotCommand","params":command});
        let first = value(&service, zuse_protocol::control::CALL, envelope.clone()).await;
        assert_eq!(
            value(&service, zuse_protocol::control::CALL, envelope).await,
            first
        );
        let coordinator = first["coordinatorChatId"].as_str().unwrap();
        let row = core.workspace.chat(coordinator).unwrap().unwrap();
        assert!(row.config.unwrap().plan, "coordinators run in plan mode");
        let grant = core.zots.grant(coordinator);
        assert!(service.handle(zuse_protocol::control::CALL,serde_json::json!({"token":grant,"method":"ZotFiles","params":{"zotId":"release"}})).await.is_err(),"idle sessions cannot use retained credentials");
        let files = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZotFiles","params":{"zotId":"release"}}),
        )
        .await;
        assert_eq!(files.as_array().unwrap().len(), 4);
        let mut changed = core.zots.get("release").unwrap().config;
        changed.coordinator_model.model = Some("selected-model".into());
        value(&service,"ZotCommand",serde_json::json!({"commandId":"configure","zotId":"release","expectedRevision":null,"action":{"type":"configure","config":changed}})).await;
        value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZotCommand","params":command}),
        )
        .await;
        assert_eq!(
            core.workspace
                .chat_config(coordinator)
                .unwrap()
                .model
                .as_deref(),
            Some("selected-model"),
            "replaying creation must not restore an obsolete model"
        );
        assert!(
            service
                .handle(
                    zuse_protocol::control::CALL,
                    serde_json::json!({"token":token,"method":"SignOut"})
                )
                .await
                .is_err(),
            "private operations are not exposed through the public API"
        );
        core.shutdown().await;
    }
}
