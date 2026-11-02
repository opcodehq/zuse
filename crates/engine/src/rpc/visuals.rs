//! Device-scoped rendering sessions for persisted transcript visuals.
use super::*;
use zuse_protocol::visual_session as wire;
impl EngineRpc {
    pub(super) async fn visual_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        if params["targetDeviceId"].as_str() != Some(self.engine_info.device_id.as_str()) {
            return Err(RpcError::BadParams(
                "Visual execution host does not match.".into(),
            ));
        }
        let failed = |error: crate::EngineError| RpcError::Failed(error.to_string());
        match method {
            methods::OPEN_VISUAL => {
                let p: wire::Open = parse_params(params)?;
                let chat = self
                    .workspace
                    .chat(&p.chat_id)
                    .map_err(failed)?
                    .ok_or_else(|| RpcError::Failed("Chat not found".into()))?;
                if chat.device_id != self.engine_info.device_id {
                    return Err(RpcError::BadParams(
                        "Visual belongs to another computer.".into(),
                    ));
                }
                let doc = self.doc_host.open(&p.chat_id).map_err(failed)?;
                let entries = doc
                    .doc()
                    .read_entries()
                    .map_err(|error| RpcError::Failed(error.to_string()))?;
                let found=entries.iter().any(|entry|entry.parts.iter().any(|part|matches!(part,zuse_model::MessagePart::Visual{id,..} if id==&p.visual_id)));
                if !found {
                    return Err(RpcError::Failed(
                        "Visual not found in this conversation.".into(),
                    ));
                }
                let uploads = self.uploads.clone();
                let id = p.visual_id;
                let (script, assets) = tokio::task::spawn_blocking({
                    let id = id.clone();
                    move || uploads.load_visual(&id)
                })
                .await
                .map_err(|_| RpcError::Failed("Visual could not be loaded.".into()))?
                .map_err(failed)?;
                Ok(RpcReply::Stream(self.visuals.open(script, id, assets)?))
            }
            methods::CONTROL_VISUAL => {
                RpcReply::value(&self.visuals.control(parse_params(params)?).await?)
            }
            methods::DRAW_VISUAL => {
                RpcReply::value(&self.visuals.draw(parse_params(params)?).await?)
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
    async fn visual_open_requires_membership_and_the_execution_host() {
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
        let id = format!("v:{}", uuid::Uuid::new_v4());
        let path = core
            .uploads
            .store_visual(&id, "return ui.text {'Hello'}")
            .unwrap();
        let rpc = core.rpc_service();
        let params =
            serde_json::json!({"chatId":"chat","visualId":id,"targetDeviceId":core.device_id});
        assert!(
            rpc.visual_rpc(methods::OPEN_VISUAL, params.clone())
                .await
                .is_err()
        );
        let doc = core.doc_host.open("chat").unwrap();
        doc.doc()
            .push_message(&zuse_model::SessionMessageEntry {
                id: "visual-entry".into(),
                role: zuse_model::MessageRole::Assistant,
                parts: vec![zuse_model::MessagePart::Visual {
                    id: id.clone(),
                    path,
                    title: "Visual".into(),
                }],
                created_at: 0,
                device_id: core.device_id.clone(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();

        let mut foreign = params.clone();
        foreign["targetDeviceId"] = serde_json::json!("other-host");
        assert!(rpc.visual_rpc(methods::OPEN_VISUAL, foreign).await.is_err());
        let RpcReply::Stream(mut stream) =
            rpc.visual_rpc(methods::OPEN_VISUAL, params).await.unwrap()
        else {
            panic!("expected owner stream")
        };
        assert_eq!(
            stream.next().await.unwrap()["tree"]["kind"]["Text"]["spans"][0]["text"],
            "Hello"
        );
        drop(stream);
        core.shutdown().await;
    }
}
