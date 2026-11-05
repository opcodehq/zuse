//! Import is host-owned, durable and independent of sending a provider turn.
use super::*;
use sha2::Digest;
use zuse_protocol::external_chat as wire;

fn failed(error: impl std::fmt::Display) -> RpcError {
    RpcError::Failed(error.to_string())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Receipt {
    request: wire::ImportRequest,
    result: Option<wire::Imported>,
}
fn write_receipt(path: &std::path::Path, receipt: &Receipt) -> Result<(), RpcError> {
    let bytes = serde_json::to_vec(receipt).map_err(failed)?;
    let temporary = path.with_extension("pending");
    std::fs::write(&temporary, bytes)
        .and_then(|_| std::fs::rename(&temporary, path))
        .map_err(failed)
}
impl EngineRpc {
    pub(super) async fn external_chats(
        &self,
        method: &str,
        mut params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        // Check before discovery, payload parsing, receipts or reading CLI files.
        if !self.doc_host.native_provider_import_allowed() {
            return Err(RpcError::Failed("Native provider history import is unavailable on managed cloud computers; use the retained Zuse conversation".into()));
        }
        if let Some(params) = params.as_object_mut() {
            params.remove("targetDeviceId");
        }
        if method == wire::LIST {
            let rows = tokio::task::spawn_blocking(|| {
                crate::external_chats::discover()
                    .into_iter()
                    .map(|c| c.row)
                    .take(50)
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(failed)?;
            return RpcReply::value(&rows);
        }
        let mut request: wire::ImportRequest = parse_params(params)?;
        let uuid = uuid::Uuid::parse_str(&request.request_id)
            .map_err(|_| RpcError::BadParams("Invalid import request ID".into()))?;
        request.request_id = uuid.to_string();
        let this = self.clone();
        tokio::spawn(Self::import_external_chat(this, request, uuid))
            .await
            .map_err(failed)?
    }
    async fn import_external_chat(
        this: Self,
        request: wire::ImportRequest,
        uuid: uuid::Uuid,
    ) -> Result<RpcReply, RpcError> {
        let gate = this
            .repos
            .git_workspace_gate(&format!("external-import:{uuid}"))
            .await;
        let _guard = gate.lock().await;
        let directory = this.repos.data_dir().join("external-chats");
        std::fs::create_dir_all(&directory).map_err(failed)?;
        let path = directory.join(format!("{uuid}.json"));
        let mut receipt = match std::fs::read(&path) {
            Ok(bytes) => {
                let receipt: Receipt = serde_json::from_slice(&bytes).map_err(failed)?;
                if receipt.request != request {
                    return Err(RpcError::BadParams(
                        "This request ID belongs to another imported chat".into(),
                    ));
                }
                if let Some(result) = receipt.result.as_ref() {
                    if this
                        .workspace
                        .chat(&result.chat.id)
                        .map_err(failed)?
                        .is_none()
                    {
                        return Err(RpcError::Failed("The imported chat was deleted. Open Import chat again to start another import.".into()));
                    }
                    return RpcReply::value(&result);
                }
                receipt
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let receipt = Receipt {
                    request: request.clone(),
                    result: None,
                };
                write_receipt(&path, &receipt)?;
                receipt
            }
            Err(error) => return Err(failed(error)),
        };
        let chat_id = format!("imported_{uuid}");
        let existing = this.workspace.chat(&chat_id).map_err(failed)?;
        let project_id = existing.as_ref().and_then(|chat| chat.space_id.as_deref());
        if let Some(chat) = existing
            .as_ref()
            .filter(|chat| chat.harness_session_id.is_some())
        {
            if let Some(project) = project_id.and_then(|id| this.workspace.space(id).ok().flatten())
            {
                receipt.result = Some(wire::Imported {
                    chat: chat.clone(),
                    project,
                });
                write_receipt(&path, &receipt)?;
                return RpcReply::value(receipt.result.as_ref().unwrap());
            }
        }
        let id = request.id.clone();
        let mut conversation = tokio::task::spawn_blocking(move || {
            let source = crate::external_chats::discover()
                .into_iter()
                .find(|c| c.row.id == id && c.row.importable())
                .ok_or_else(|| {
                    "That provider chat is no longer available. Reopen Import chat to refresh."
                        .to_owned()
                })?;
            crate::external_chats::read(&source.source, source.row.provider, true)
        })
        .await
        .map_err(failed)?
        .map_err(RpcError::Failed)?;
        this.doc_host
            .mask_imported_messages(&mut conversation.messages);
        if conversation.messages.is_empty() {
            return Err(RpcError::Failed(
                "No readable messages were found in that provider chat.".into(),
            ));
        }
        let cwd = std::fs::canonicalize(&conversation.row.project_path)
            .map_err(failed)?
            .to_string_lossy()
            .into_owned();
        let spaces = this.workspace.read_spaces().map_err(failed)?;
        let device = this.doc_host.device_id();
        let project_id = spaces
            .iter()
            .find(|space| {
                space.device_id == device
                    && std::fs::canonicalize(&space.path)
                        .ok()
                        .is_some_and(|path| path.to_string_lossy() == cwd)
            })
            .map(|space| space.id.clone())
            .unwrap_or_else(|| {
                format!(
                    "import-project-{:x}",
                    sha2::Sha256::digest(format!("{device}\0{cwd}"))
                )
            });
        let handle = this.doc_host.open_local(&chat_id).map_err(failed)?;
        for (entry_id, payload) in &conversation.sidecars {
            let (output, diff) = this
                .doc_host
                .persist_tool_sidecar(&chat_id, entry_id, payload)
                .map_err(failed)?;
            if let Some(message) = conversation.messages.iter_mut().find(|m| m.id == *entry_id) {
                for part in &mut message.parts {
                    if let zuse_doc::MessagePart::Tool {
                        id,
                        output_ref,
                        diff_ref,
                        output_bytes,
                        ..
                    } = part
                    {
                        if *id == payload.part_id {
                            *output_ref = output.clone();
                            *diff_ref = diff.clone();
                            *output_bytes = payload.output.as_ref().map(|text| text.len() as u64);
                        }
                    }
                }
            }
        }
        for message in &mut conversation.messages {
            message.device_id = device.to_owned();
        }
        handle
            .import_transcript(|| -> Result<(), zuse_doc::DocError> {
                let existing: std::collections::HashSet<_> = handle
                    .doc()
                    .read_entries()?
                    .into_iter()
                    .map(|message| message.id)
                    .collect();
                for message in &conversation.messages {
                    if !existing.contains(&message.id) {
                        handle.doc().push_message(message)?;
                    }
                }
                Ok(())
            })
            .map_err(failed)?;
        this.doc_host.flush_chat_checked(&handle).map_err(failed)?;
        let git = std::path::Path::new(&cwd).join(".git").exists();
        this.workspace
            .create_space(
                &project_id,
                device,
                &cwd,
                Some(conversation.row.project_name.clone()),
                git,
            )
            .map_err(failed)?;
        if git {
            this.repos.add(&cwd).await.map_err(failed)?;
        }
        this.workspace
            .create_chat(
                &chat_id,
                Some(&project_id),
                Some(device),
                Some(ChatConfig {
                    harness: conversation.row.provider,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
                }),
                Some(cwd.clone()),
            )
            .map_err(failed)?;
        this.workspace
            .finish_external_import(
                &chat_id,
                &conversation.row.title,
                &conversation.cursor,
                &cwd,
                conversation.row.provider,
                &conversation.row.preview,
                conversation.row.updated_at,
            )
            .map_err(failed)?;
        let result = wire::Imported {
            chat: this
                .workspace
                .chat(&chat_id)
                .map_err(failed)?
                .ok_or_else(|| RpcError::Failed("Imported chat was removed".into()))?,
            project: this.workspace.space(&project_id).map_err(failed)?.unwrap(),
        };
        receipt.result = Some(result.clone());
        write_receipt(&path, &receipt)?;
        RpcReply::value(&result)
    }
}

#[cfg(test)]
mod cloud_policy_tests {
    use super::*;
    #[tokio::test]
    async fn cold_resume_managed_import_policy_rejects_list_and_invalid_import_before_native_discovery()
     {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble(
            root.path(),
            std::sync::Arc::new(crate::registry::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        assert!(
            core.doc_host.native_provider_import_allowed(),
            "desktop default retains imports"
        );
        core.doc_host.disable_native_provider_import();
        for method in [wire::LIST, wire::IMPORT] {
            let error = match core
                .rpc_service()
                .external_chats(method, serde_json::json!({"not":"an import payload"}))
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("managed CLI files must stay private"),
            };
            assert!(
                error.to_string().contains("unavailable on managed cloud"),
                "policy must precede payload parsing"
            );
        }
        assert!(!core.repos.data_dir().join("external-chats").exists());
        core.doc_host.shutdown_workers().await;
    }
}
