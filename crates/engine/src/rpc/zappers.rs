use super::*;
use zuse_protocol::zappers::ZapperAction;

impl EngineRpc {
    pub(super) async fn zapper_command(
        &self,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        let actor = params["_zapperActor"].as_str().map(str::to_owned);
        let worker = params["_zapperWorkerChatId"].as_str().map(str::to_owned);
        let command = zuse_protocol::zappers::parse_command(params).map_err(RpcError::BadParams)?;
        let zappers = self
            .zappers
            .as_ref()
            .ok_or_else(|| RpcError::Capability("zappers.v1".into()))?;
        let failed = |e: anyhow::Error| RpcError::Failed(e.to_string());
        let from_coordinator = actor.is_some() && worker.is_none();
        if let Some(worker) = worker {
            let issue_id = match &command.action {
                ZapperAction::CommentIssue { issue_id, .. }
                | ZapperAction::LinkIssuePullRequest { issue_id, .. } => issue_id,
                _ => return Err(RpcError::Capability("worker.issue_scope".into())),
            };
            let zapper = zappers.get(&command.zapper_id).map_err(failed)?;
            if !zapper.workers.iter().any(|w| w.chat_id == worker)
                || !zapper
                    .issues
                    .iter()
                    .any(|i| i.id == *issue_id && i.threads.contains(&worker))
            {
                return Err(RpcError::Capability("worker.issue_scope".into()));
            }
        }

        if let Some(result) = zappers.receipt(&command).map_err(failed)? {
            return RpcReply::value(&result);
        }
        if let ZapperAction::Create { config } | ZapperAction::Configure { config } =
            &command.action
        {
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
        if let ZapperAction::AdoptWorker { worker } = &command.action {
            let chat = self
                .workspace
                .chat(&worker.chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::BadParams("worker conversation does not exist".into()))?;
            if chat.device_id != worker.device_id
                || chat.space_id.as_deref() != Some(&worker.space_id)
                || chat.parent_chat_id.is_some()
                || zappers.coordinator(&chat.id).map_err(failed)?.is_some()
            {
                return Err(RpcError::BadParams(
                    "worker must be an ordinary conversation in the selected repository".into(),
                ));
            }
        }
        if let ZapperAction::Delegate {
            worker,
            model,
            base,
            ..
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
            if !zuse_protocol::zappers::worker_chat_id_for(&worker.chat_id, &command.command_id)
                || base.trim().is_empty()
            {
                return Err(RpcError::BadParams(
                    "delegate chatId must be zapper-worker-<commandId>, with an explicit base ref"
                        .into(),
                ));
            }
            let _: HarnessId = serde_json::from_value(serde_json::json!(model.harness))
                .map_err(|e| RpcError::BadParams(e.to_string()))?;
        }
        let old = zappers.get(&command.zapper_id).ok();
        if matches!(
            command.action,
            ZapperAction::FreshConversation | ZapperAction::Delete
        ) && old
            .as_ref()
            .is_some_and(|z| self.sessions.turn_in_flight(&z.coordinator_chat_id))
        {
            return Err(RpcError::Failed(
                "stop the active coordinator turn first".into(),
            ));
        }
        let result = zappers
            .command_as(command.clone(), actor.as_deref(), from_coordinator)
            .map_err(failed)?;
        let current = result.as_ref().and_then(|z| zappers.get(&z.id).ok());
        if let Some(zapper) = &current {
            let harness: HarnessId =
                serde_json::from_value(serde_json::json!(zapper.config.coordinator_model.harness))
                    .map_err(|e| RpcError::BadParams(e.to_string()))?;
            let cwd = zappers
                .workspace(&zapper.id)
                .map_err(failed)?
                .to_string_lossy()
                .into_owned();
            let previous = old
                .as_ref()
                .filter(|o| o.coordinator_chat_id != zapper.coordinator_chat_id)
                .and_then(|o| self.workspace.chat_config(&o.coordinator_chat_id));
            self.workspace
                .create_chat_with_parent(
                    &zapper.coordinator_chat_id,
                    None,
                    Some(&zapper.config.home_device_id),
                    Some(zuse_protocol::ChatConfig {
                        harness: previous.as_ref().map_or(harness, |c| c.harness),
                        model: previous.as_ref().map_or_else(
                            || zapper.config.coordinator_model.model.clone(),
                            |c| c.model.clone(),
                        ),
                        reasoning: None,
                        model_options: Default::default(),
                        sandbox: zuse_protocol::SandboxLevel::Auto,
                        plan: false,
                    }),
                    Some(cwd),
                    None,
                )
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            self.workspace
                .rename_chat(&zapper.coordinator_chat_id, &zapper.config.name)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            let mut config = self
                .workspace
                .chat_config(&zapper.coordinator_chat_id)
                .unwrap_or(zuse_protocol::ChatConfig {
                    harness,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: zuse_protocol::SandboxLevel::Auto,
                    plan: false,
                });
            // The conversation's own model picker chooses the model after
            // creation; only an explicit model change through the API applies.
            if let ZapperAction::Configure { config: wanted } = &command.action {
                if old
                    .as_ref()
                    .is_some_and(|o| o.config.coordinator_model != wanted.coordinator_model)
                {
                    config.harness = harness;
                    config.model = zapper.config.coordinator_model.model.clone();
                }
            }
            config.sandbox = zuse_protocol::SandboxLevel::Auto;
            config.plan = false;
            self.workspace
                .set_chat_config(&zapper.coordinator_chat_id, &config)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
        }
        RpcReply::value(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EngineCore, EngineProfile};
    use zuse_protocol::zappers::*;

    #[tokio::test]
    async fn worker_issue_commands_are_limited_to_linked_issues() {
        let dir = tempfile::tempdir().unwrap();
        let core = EngineCore::assemble_with_profile(
            EngineProfile::local(dir.path()).unwrap(),
            std::sync::Arc::new(HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "repo",
                &core.device_id,
                dir.path().to_str().unwrap(),
                None,
                false,
            )
            .unwrap();
        let service = core.rpc_service();
        value(&service, "Mutate", serde_json::json!({"op":"createChat","chatId":"worker","deviceId":core.device_id,"spaceId":"repo"})).await;
        value(&service, "ZapperCommand", serde_json::json!({"commandId":"create","zapperId":"release","expectedRevision":null,"action":{"type":"create","config":{"name":"Release","purpose":"Coordinate","homeDeviceId":core.device_id,"repositories":[{"deviceId":core.device_id,"spaceId":"repo"}],"coordinatorModel":{"harness":"mock","model":null},"workerDeviceId":core.device_id,"maxConcurrentWorkers":1,"maxFollowups":3}}})).await;
        value(&service, "ZapperCommand", serde_json::json!({"commandId":"adopt","zapperId":"release","expectedRevision":null,"action":{"type":"adoptWorker","worker":{"chatId":"worker","deviceId":core.device_id,"spaceId":"repo","objective":"Work","followups":0}}})).await;
        for id in ["linked", "unlinked"] {
            value(&service,"ZapperCommand",serde_json::json!({"commandId":format!("create-{id}"),"zapperId":"release","expectedRevision":null,"action":{"type":"createIssue","issueId":id,"details":{"title":"Work","description":"","dueDate":null}}})).await;
        }
        value(&service,"ZapperCommand",serde_json::json!({"commandId":"link","zapperId":"release","expectedRevision":null,"action":{"type":"linkIssueThread","issueId":"linked","chatId":"worker"}})).await;
        let worker_command = |id: &str, issue: &str| serde_json::json!({"commandId":id,"zapperId":"release","expectedRevision":null,"_zapperActor":"worker","_zapperWorkerChatId":"worker","action":{"type":"commentIssue","issueId":issue,"text":"Tests passed"}});
        value(
            &service,
            "ZapperCommand",
            worker_command("report", "linked"),
        )
        .await;
        assert!(
            service
                .handle("ZapperCommand", worker_command("foreign", "unlinked"))
                .await
                .is_err()
        );
        let mut forbidden = worker_command("close", "linked");
        forbidden["action"] = serde_json::json!({"type":"editIssue","issueId":"linked","details":{"title":"Closed","description":"","status":"done","dueDate":null}});
        assert!(service.handle("ZapperCommand", forbidden).await.is_err());
        forbidden["action"] = serde_json::json!({"type":"linkIssueRelation","issueId":"linked","kind":"blocks","targetId":"unlinked"});
        assert!(service.handle("ZapperCommand", forbidden).await.is_err());
        let zapper = core.zappers.get("release").unwrap();
        assert_eq!(
            zapper.issues[0].activity.last().unwrap().actor.as_deref(),
            Some("worker")
        );
        assert_eq!(zapper.issues[1].activity.len(), 1);
        core.shutdown().await;
    }

    /// Clients, peers and coordinators from before the rename keep working:
    /// old method names, `zotId` and the old internal fields are served as
    /// current ones (legacy, remove after one release).
    #[tokio::test]
    async fn pre_rename_method_names_and_fields_are_still_served() {
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
        core.workspace
            .create_space(
                "repo",
                &core.device_id,
                dir.path().to_str().unwrap(),
                None,
                false,
            )
            .unwrap();
        let service = core.rpc_service();
        value(&service, "Mutate", serde_json::json!({"op":"createChat","chatId":"worker","deviceId":core.device_id,"spaceId":"repo"})).await;
        let created = value(&service, "ZotCommand", serde_json::json!({"commandId":"create","zotId":"release","expectedRevision":null,"action":{"type":"create","config":{"name":"Release","purpose":"Coordinate","homeDeviceId":core.device_id,"repositories":[{"deviceId":core.device_id,"spaceId":"repo"}],"coordinatorModel":{"harness":"mock","model":null},"workerDeviceId":core.device_id,"maxConcurrentWorkers":1,"maxFollowups":3}}})).await;
        assert_eq!(created["coordinatorChatId"], "zapper-release-0");
        value(&service, "ZotCommand", serde_json::json!({"commandId":"adopt","zotId":"release","expectedRevision":null,"action":{"type":"adoptWorker","worker":{"chatId":"worker","deviceId":core.device_id,"spaceId":"repo","objective":"Work","followups":0}}})).await;
        value(&service, "ZotCommand", serde_json::json!({"commandId":"issue","zotId":"release","expectedRevision":null,"action":{"type":"createIssue","issueId":"linked","details":{"title":"Work","description":"","dueDate":null}}})).await;
        value(&service, "ZotCommand", serde_json::json!({"commandId":"link","zotId":"release","expectedRevision":null,"action":{"type":"linkIssueThread","issueId":"linked","chatId":"worker"}})).await;
        // A worker's report as an older peer engine forwards it.
        value(&service, "ZotCommand", serde_json::json!({"commandId":"report","zotId":"release","expectedRevision":null,"_zotActor":"worker","_zotWorkerChatId":"worker","action":{"type":"commentIssue","issueId":"linked","text":"Tests passed"}})).await;
        let zapper = core.zappers.get("release").unwrap();
        assert_eq!(
            zapper.issues[0].activity.last().unwrap().actor.as_deref(),
            Some("worker")
        );
        // Old names read the same data, in the shape older clients parse.
        let files = value(&service, "ZotFiles", serde_json::json!({"zotId":"release"})).await;
        assert!(!files.as_array().unwrap().is_empty());
        let events = value(
            &service,
            "ZotEvents",
            serde_json::json!({"zotId":"release"}),
        )
        .await;
        let current = value(
            &service,
            "ZapperEvents",
            serde_json::json!({"zapperId":"release"}),
        )
        .await;
        assert_eq!(
            events.as_array().unwrap().len(),
            current.as_array().unwrap().len()
        );
        for (old, new) in events
            .as_array()
            .unwrap()
            .iter()
            .zip(current.as_array().unwrap())
        {
            assert_eq!(old["zotId"], "release");
            assert!(old.get("zapperId").is_none());
            assert_eq!(new["zapperId"], "release");
        }
        let board = value(
            &service,
            "ZotIssue",
            serde_json::json!({"zotId":"release","issueId":"ZAP-1"}),
        )
        .await;
        assert_eq!(board["id"], "linked");
        // Control calls from tools started before the rename.
        let token = std::fs::read_to_string(store_root.join("zappers/control-token")).unwrap();
        let info = value(
            &service,
            zuse_protocol::control::API_INFO,
            serde_json::json!({}),
        )
        .await;
        assert!(
            info["capabilities"]
                .as_array()
                .unwrap()
                .contains(&"zots.v1".into())
        );
        let via_call = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZotFiles","params":{"zotId":"release"}}),
        )
        .await;
        assert_eq!(via_call, files);
        core.shutdown().await;
    }

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
        let config = ZapperConfig {
            name: "Release".into(),
            purpose: "Coordinate".into(),
            home_device_id: core.device_id.clone(),
            repositories: vec![ZapperRepository {
                device_id: core.device_id.clone(),
                space_id: "repo".into(),
            }],
            coordinator_model: ZapperModel {
                harness: "mock".into(),
                model: None,
            },
            worker_device_id: core.device_id.clone(),
            max_concurrent_workers: 1,
            max_followups: 3,
            avatar: None,
            avatar_image: None,
        };
        value(&service,"ZapperCommand",serde_json::json!({"commandId":"create","zapperId":"release","expectedRevision":null,"action":{"type":"create","config":config}})).await;
        let launch = serde_json::json!({"commandId":"launch","zapperId":"release","expectedRevision":null,"action":{"type":"delegate","worker":{"chatId":"zapper-worker-launch","deviceId":core.device_id,"spaceId":"repo","objective":"Inspect the repository","followups":0},"model":{"harness":"mock","model":null},"base":"main"}});
        value(&service, "ZapperCommand", launch.clone()).await;
        value(&service, "ZapperCommand", launch).await;
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if core
                    .zappers
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
        let chat = core
            .workspace
            .chat("zapper-worker-launch")
            .unwrap()
            .unwrap();
        let cwd = std::path::PathBuf::from(chat.cwd.unwrap());
        assert_ne!(cwd, repo);
        assert!(cwd.starts_with(worktrees));
        let reports = core
            .zappers
            .events("release")
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "settled")
            .count();
        assert_eq!(reports, 1);
        assert_eq!(core.zappers.get("release").unwrap().workers.len(), 1);
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
    async fn zapper_api_receipts_files_and_auth_share_engine_authority() {
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
        let token = std::fs::read_to_string(store_root.join("zappers/control-token")).unwrap();
        let info = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ControlInfo"}),
        )
        .await;
        assert_eq!(info["apiVersion"], 1);
        let config = ZapperConfig {
            name: "Release".into(),
            purpose: "Coordinate".into(),
            home_device_id: core.device_id.clone(),
            repositories: vec![],
            coordinator_model: ZapperModel {
                harness: "mock".into(),
                model: None,
            },
            worker_device_id: core.device_id.clone(),
            max_concurrent_workers: 4,
            max_followups: 3,
            avatar: None,
            avatar_image: None,
        };
        let command = ZapperCommand {
            command_id: "create".into(),
            zapper_id: "release".into(),
            expected_revision: None,
            action: ZapperAction::Create { config },
        };
        let envelope = serde_json::json!({"token":token,"method":"ZapperCommand","params":command});
        let first = value(&service, zuse_protocol::control::CALL, envelope.clone()).await;
        assert_eq!(
            value(&service, zuse_protocol::control::CALL, envelope).await,
            first
        );
        let coordinator = first["coordinatorChatId"].as_str().unwrap();
        let row = core.workspace.chat(coordinator).unwrap().unwrap();
        let config = row.config.unwrap();
        assert!(!config.plan, "coordinators must be able to act");
        assert_eq!(config.sandbox, zuse_protocol::SandboxLevel::Auto);
        let grant = core.zappers.grant(coordinator);
        assert!(service.handle(zuse_protocol::control::CALL,serde_json::json!({"token":grant,"method":"ZapperFiles","params":{"zapperId":"release"}})).await.is_err(),"idle sessions cannot use retained credentials");
        let files = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZapperFiles","params":{"zapperId":"release"}}),
        )
        .await;
        assert_eq!(files.as_array().unwrap().len(), 4);
        value(&service,"ZapperCommand",serde_json::json!({"commandId":"issue","zapperId":"release","action":{"type":"createIssue","issueId":"fix","details":{"title":"Fix it","description":"d","doneCriteria":"c","outOfScope":"","status":"todo","priority":"normal","owner":{"type":"zapper"},"assigned":true,"parent":null,"dueDate":null,"labels":[],"relations":[]}}})).await;
        // Later coordinator turns carry deltas; the board is a read away.
        let board = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZapperIssue","params":{"zapperId":"release"}}),
        )
        .await;
        assert_eq!(board["issues"][0]["code"], "ZAP-1");
        assert_eq!(board["id"], "release");
        let by_code = value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZapperIssue","params":{"zapperId":"release","issueId":"ZAP-1"}}),
        )
        .await;
        assert_eq!(by_code["id"], "fix");
        let mut changed = core.zappers.get("release").unwrap().config;
        changed.coordinator_model.model = Some("selected-model".into());
        value(&service,"ZapperCommand",serde_json::json!({"commandId":"configure","zapperId":"release","expectedRevision":null,"action":{"type":"configure","config":changed}})).await;
        value(
            &service,
            zuse_protocol::control::CALL,
            serde_json::json!({"token":token,"method":"ZapperCommand","params":command}),
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
