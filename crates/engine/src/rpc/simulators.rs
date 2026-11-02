//! Device-addressed simulator RPCs. Agent operations cannot change user consent.
use super::*;
use serde_json::{Value, json};
use zuse_protocol::simulators::{FrameEvent, Input, Target};

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::SIMULATOR_STATE
            | methods::CONFIGURE_SIMULATORS
            | methods::OPEN_SIMULATOR
            | methods::CLOSE_SIMULATOR
            | methods::SIMULATOR_INPUT
            | methods::SIMULATOR_SCREENSHOT
            | methods::SIMULATOR_FRAMES
            | methods::SIMULATOR_TOOL
    )
}
impl EngineRpc {
    pub(super) async fn simulator_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> Result<RpcReply, RpcError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Request {
            chat_id: String,
            device_id: Option<String>,
            enabled: Option<bool>,
            agent_access: Option<bool>,
            #[serde(default)]
            shutdown: bool,
            input: Option<Input>,
            name: Option<String>,
            #[serde(default)]
            arguments: Value,
        }
        let p: Request = parse_params(params)?;
        let chats = self
            .workspace
            .read_chats()
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        self.simulators.retain_chats(
            &chats
                .into_iter()
                .filter(|c| c.device_id == self.engine_info.device_id)
                .map(|c| c.id)
                .collect(),
        );
        let chat = self
            .workspace
            .chat(&p.chat_id)
            .map_err(|e| RpcError::Failed(e.to_string()))?
            .ok_or_else(|| RpcError::BadParams("Chat not found".into()))?;
        if chat.device_id != self.engine_info.device_id {
            return Err(RpcError::BadParams(
                "Simulators belong to the chat's execution host".into(),
            ));
        }
        let failed = |e: anyhow::Error| RpcError::Failed(format!("{e:#}"));
        if method == methods::SIMULATOR_STATE {
            return RpcReply::value(&self.simulators.state(&p.chat_id).await);
        }
        if method == methods::CONFIGURE_SIMULATORS {
            let current = self.simulators.preferences(&p.chat_id);
            let enabled = p.enabled.unwrap_or(current.0);
            let access = p.agent_access.unwrap_or(current.1);
            self.simulators.configure(&p.chat_id, enabled, access).await;
            return RpcReply::value(&self.simulators.snapshot(&p.chat_id));
        }
        let agent = method == methods::SIMULATOR_TOOL;
        let (action, device_id, input, shutdown) = if agent {
            let name = p.name.as_deref().unwrap_or_default();
            self.simulators
                .authorize(&p.chat_id, true)
                .await
                .map_err(failed)?;
            if matches!(name, "device_open" | "device_close" | "device_input")
                && !chat
                    .config
                    .as_ref()
                    .is_some_and(|c| c.sandbox != zuse_protocol::SandboxLevel::ReadOnly)
            {
                return Err(RpcError::Failed(
                    "Device actions are blocked in plan mode".into(),
                ));
            }
            let action = match name {
                "device_list" => return RpcReply::value(&self.simulators.state(&p.chat_id).await),
                "device_open" => methods::OPEN_SIMULATOR,
                "device_close" => methods::CLOSE_SIMULATOR,
                "device_screenshot" => methods::SIMULATOR_SCREENSHOT,
                "device_input" => methods::SIMULATOR_INPUT,
                _ => return Err(RpcError::BadParams("Unknown device tool".into())),
            };
            let input = p
                .arguments
                .get("input")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| RpcError::BadParams(e.to_string()))?;
            (
                action,
                p.arguments["deviceId"].as_str().map(str::to_owned),
                input,
                p.arguments["shutdown"].as_bool().unwrap_or(false),
            )
        } else {
            (method, p.device_id, p.input, p.shutdown)
        };
        if action == methods::CLOSE_SIMULATOR && !agent && device_id.is_none() {
            self.simulators.close_chat(&p.chat_id).await;
            return RpcReply::value(&json!({"ok":true}));
        }
        let target = Target {
            chat_id: p.chat_id,
            device_id: device_id
                .ok_or_else(|| RpcError::BadParams("deviceId is required".into()))?,
        };
        match action {
            methods::OPEN_SIMULATOR => {
                RpcReply::value(&self.simulators.open(&target, agent).await.map_err(failed)?)
            }
            methods::CLOSE_SIMULATOR => {
                self.simulators
                    .close(&target, shutdown, agent)
                    .await
                    .map_err(failed)?;
                RpcReply::value(&json!({"ok":true}))
            }
            methods::SIMULATOR_INPUT => {
                self.simulators
                    .input(
                        &target,
                        input.ok_or_else(|| RpcError::BadParams("input is required".into()))?,
                        agent,
                    )
                    .await
                    .map_err(failed)?;
                RpcReply::value(&json!({"ok":true}))
            }
            methods::SIMULATOR_SCREENSHOT => RpcReply::value(
                &self
                    .simulators
                    .screenshot(&target, agent)
                    .await
                    .map_err(failed)?,
            ),
            methods::SIMULATOR_FRAMES => {
                self.simulators
                    .authorize(&target.chat_id, false)
                    .await
                    .map_err(failed)?;
                let simulators = self.simulators.clone();
                let workspace = self.workspace.clone();
                let host = self.engine_info.device_id.clone();
                // Capture only when the transport requests another item: no frame queue,
                // no detached producer, and dropping the stream cancels its subprocess.
                let stream = futures::stream::unfold(
                    (simulators, target, false, false),
                    move |(simulators, target, delay, ended)| {
                        let workspace = workspace.clone();
                        let host = host.clone();
                        async move {
                            if ended {
                                return None;
                            }
                            if delay {
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }
                            if !workspace
                                .chat(&target.chat_id)
                                .ok()
                                .flatten()
                                .is_some_and(|c| c.device_id == host)
                            {
                                simulators.forget_chat(&target.chat_id);
                            }
                            let (event, ended) = match simulators.screenshot(&target, false).await {
                                Ok(frame) => (FrameEvent::Frame { frame }, false),
                                Err(error) => (
                                    FrameEvent::Error {
                                        message: format!("{error:#}"),
                                    },
                                    true,
                                ),
                            };
                            Some((
                                serde_json::to_value(event).unwrap(),
                                (simulators, target, true, ended),
                            ))
                        }
                    },
                )
                .boxed();
                Ok(RpcReply::Stream(stream))
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn routes_to_chat_host_and_rejects_unconsented_agents_and_plan_mutations() {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(root.path()).unwrap(),
            Arc::new(HarnessRegistry::new()),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        core.workspace
            .create_chat("chat", None, Some(&core.device_id), None, None)
            .unwrap();
        core.workspace
            .create_chat("remote", None, Some("another-host"), None, None)
            .unwrap();
        let simulator = zuse_simulator::Simulators::new(
            Arc::new(Fixture),
            zuse_simulator::Toolchain {
                macos: true,
                ..Default::default()
            },
        );
        let rpc = core
            .rpc_service()
            .as_ref()
            .clone()
            .with_simulators(simulator.clone());
        assert!(
            rpc.simulator_rpc(
                methods::SIMULATOR_TOOL,
                json!({"chatId":"chat","name":"device_list"})
            )
            .await
            .is_err()
        );
        assert!(
            rpc.simulator_rpc(methods::SIMULATOR_STATE, json!({"chatId":"remote"}))
                .await
                .is_err()
        );
        assert!(
            rpc.simulator_rpc(methods::SIMULATOR_STATE, json!({"chatId":"missing"}))
                .await
                .is_err()
        );
        simulator.configure("chat", true, true).await;
        let result = rpc
            .simulator_rpc(
                methods::SIMULATOR_TOOL,
                json!({"chatId":"chat","name":"device_open","arguments":{"deviceId":"anything"}}),
            )
            .await;
        assert!(matches!(result,Err(RpcError::Failed(message)) if message.contains("plan mode")));
        let RpcReply::Value(updated) = rpc
            .simulator_rpc(
                methods::CONFIGURE_SIMULATORS,
                json!({"chatId":"chat","agentAccess":false}),
            )
            .await
            .unwrap()
        else {
            panic!("configure must return state")
        };
        assert_eq!(updated["enabled"], true);
        assert_eq!(updated["agentAccess"], false);
        let RpcReply::Value(updated) = rpc
            .simulator_rpc(
                methods::CONFIGURE_SIMULATORS,
                json!({"chatId":"chat","enabled":false}),
            )
            .await
            .unwrap()
        else {
            panic!("configure must return state")
        };
        assert_eq!(updated["enabled"], false);
        assert!(super::super::forwardable(methods::SIMULATOR_TOOL));
        assert_eq!(
            super::super::forward_deadline(methods::OPEN_SIMULATOR),
            Duration::from_secs(480)
        );
        assert!(super::super::is_stream_method(methods::SIMULATOR_FRAMES));
    }
    struct Fixture;
    #[async_trait]
    impl zuse_simulator::Runner for Fixture {
        async fn run(&self, _: &str, args: &[String], _: Duration) -> anyhow::Result<Vec<u8>> {
            if args.get(1).map(String::as_str) == Some("list") {
                return Ok(serde_json::to_vec(
                    &json!({"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{"udid":"AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE","name":"Fixture iPhone","isAvailable":true,"state":"Booted"}]}}),
                )?);
            }
            if args.iter().any(|a| a == "screenshot") {
                let mut png = vec![0; 33];
                png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
                png[12..16].copy_from_slice(b"IHDR");
                png[16..20].copy_from_slice(&1u32.to_be_bytes());
                png[20..24].copy_from_slice(&2u32.to_be_bytes());
                return Ok(png);
            }
            Ok(vec![])
        }
        async fn start(&self, _: &str, _: &[String]) -> anyhow::Result<()> {
            panic!("fixture must not start an emulator")
        }
    }
    #[tokio::test]
    async fn simulator_rpc_streams_frames_and_stops_after_consent_is_revoked() {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(root.path()).unwrap(),
            Arc::new(HarnessRegistry::new()),
            HarnessId::Codex,
            None,
        )
        .unwrap();
        core.workspace
            .create_chat("chat", None, Some(&core.device_id), None, None)
            .unwrap();
        let sim = zuse_simulator::Simulators::new(
            Arc::new(Fixture),
            zuse_simulator::Toolchain {
                macos: true,
                ..Default::default()
            },
        );
        let rpc = core.rpc_service().as_ref().clone().with_simulators(sim);
        let target = json!({"chatId":"chat","deviceId":"AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"});
        rpc.handle(
            methods::CONFIGURE_SIMULATORS,
            json!({"chatId":"chat","enabled":true,"agentAccess":true}),
        )
        .await
        .unwrap();
        rpc.handle(methods::OPEN_SIMULATOR, target.clone())
            .await
            .unwrap();
        let RpcReply::Stream(mut frames) = rpc
            .handle(methods::SIMULATOR_FRAMES, target.clone())
            .await
            .unwrap()
        else {
            panic!("expected frame stream")
        };
        let frame = frames.next().await.unwrap();
        assert_eq!(frame["type"], "frame");
        assert_eq!(frame["frame"]["width"], 1);
        let RpcReply::Value(screenshot)=rpc.handle(methods::SIMULATOR_TOOL,json!({"chatId":"chat","name":"device_screenshot","arguments":{"deviceId":target["deviceId"]}})).await.unwrap() else {panic!("expected screenshot")};
        assert_eq!(screenshot["height"], 2);
        rpc.handle(
            methods::CONFIGURE_SIMULATORS,
            json!({"chatId":"chat","enabled":false,"agentAccess":false}),
        )
        .await
        .unwrap();
        assert_eq!(frames.next().await.unwrap()["type"], "error");
        assert!(frames.next().await.is_none());
        rpc.handle(
            methods::CONFIGURE_SIMULATORS,
            json!({"chatId":"chat","enabled":true,"agentAccess":true}),
        )
        .await
        .unwrap();
        rpc.handle(methods::OPEN_SIMULATOR, target.clone())
            .await
            .unwrap();
        let RpcReply::Stream(mut frames) =
            rpc.handle(methods::SIMULATOR_FRAMES, target).await.unwrap()
        else {
            panic!("expected frame stream")
        };
        assert_eq!(frames.next().await.unwrap()["type"], "frame");
        core.workspace.delete_chat("chat").unwrap();
        assert_eq!(frames.next().await.unwrap()["type"], "error");
        assert!(frames.next().await.is_none());
    }
}
